// The window. It asks the app for a snapshot several times a second, draws it, and sends
// back what the person does as commands. It keeps no record of its own about connections.
//
// Names of computers come from other computers, so they are never put into the page as
// HTML: everything goes in as text.

import {
  blockersAt,
  boundsOf,
  box,
  contactSegments,
  fitView,
  formatSize,
  isPlaced,
  monitorFacts,
  overlapRegions,
  panBy,
  pairingAddress,
  shareAddress,
  snapPosition,
  statusBarOf,
  statusOf,
  tabsFor,
  toCanvas,
  toDesktop,
  unreachable,
  zoomAt,
} from './logic.js';

const POLL_MS = 250;

// ---- talking to the app ----

const tauri = window.__TAURI__;

async function api(cmd, args = {}) {
  if (tauri && tauri.core) return tauri.core.invoke('call', { cmd, args });

  // Outside the app (the development server), the same commands are plain HTTP.
  const response = await fetch('/call', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ cmd, args }),
  });
  const body = await response.json();
  if (!response.ok) throw new Error(body.error || response.statusText);
  return body;
}

// ---- state ----

let snap = null;
let tab = null;
let lastKey = '';
let overlayKey = null;
let builtView = null;
let builtSettingsMode = null;
let forgetting = null; // the computer the person is being asked to forget
let pairAddress = ''; // what has been typed as the address to pair with
let localError = null;
let selection = new Set(); // the computers that are selected, by name
let pickedMonitor = null; // which monitor of the only selected computer the person pressed
let spaceDown = false; // while Space is held, dragging the canvas moves the view

/** The computer whose monitors and details are shown: the one, when just one is selected. */
function sole() {
  return selection.size === 1 ? [...selection][0] : null;
}
let drag = null; // the screen being carried, while the person is dragging one
let renderPending = false;

/** A drag that has lasted this long is assumed to have been lost, so drawing resumes. */
const STALE_DRAG_MS = 20000;

const $ = (id) => document.getElementById(id);

/** Builds an element. Children that are strings become text, never HTML. */
function h(tag, props = {}, ...children) {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === null || value === false) continue;
    if (key === 'class') element.className = value;
    else if (key === 'text') element.textContent = value;
    else if (key === 'data') Object.assign(element.dataset, value);
    else if (key === 'on') {
      for (const [event, handler] of Object.entries(value)) element.addEventListener(event, handler);
    } else if (key in element) element[key] = value;
    else element.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child !== null && child !== undefined && child !== false) element.append(child);
  }
  return element;
}

async function act(cmd, args) {
  try {
    localError = null;
    apply(await api(cmd, args));
  } catch (error) {
    localError = String(error);
    render();
  }
}

async function refresh() {
  try {
    const next = await api('snapshot');
    if (localError && localError.startsWith('앱에 연결')) localError = null;
    apply(next);
  } catch (error) {
    const message = `앱에 연결할 수 없습니다: ${error}`;
    if (localError !== message) {
      localError = message;
      if (snap) render();
    }
  }
}

function apply(next) {
  const key = JSON.stringify(next);
  if (key === lastKey) {
    if (localError !== null) render();
    return;
  }
  lastKey = key;
  snap = next;
  if (tab === null) tab = tabsFor()[0][0];

  // Rebuilding the page would cancel a drag in progress, so it waits until the drag is over.
  if (drag && Date.now() - drag.startedAt > STALE_DRAG_MS) endDrag();
  if (drag) renderPending = true;
  else render();
}

// ---- drawing ----

function render() {
  if (!snap) return;
  document.body.classList.toggle('settings-open', tab === 'settings');
  const focus = rememberFocus();
  renderBanners();
  renderTabs();
  renderView();
  renderStatusbar();
  renderOverlay();
  restoreFocus(focus);
}

function rememberFocus() {
  const element = document.activeElement;
  if (!element || !element.id) return null;
  return { id: element.id, start: element.selectionStart, end: element.selectionEnd };
}

function restoreFocus(focus) {
  if (!focus) return;
  const element = $(focus.id);
  if (!element || document.activeElement === element) return;
  element.focus();
  if (focus.start !== null && element.setSelectionRange) element.setSelectionRange(focus.start, focus.end);
}

/** The bar at the bottom: whether this computer is sharing, how many are online, and start/stop. */
function renderStatusbar() {
  const { items, address } = statusBarOf(snap);
  const toggle = h('button', {
    type: 'button',
    class: `toggle ${snap.running ? 'danger' : 'primary'}`,
    title: snap.running ? '중지' : '시작',
    on: { click: () => act(snap.running ? 'stop' : 'start') },
  }, icon(snap.running ? 'stop' : 'play', 12, 'inline'), snap.running ? '중지' : '시작');

  $('statusbar').replaceChildren(
    ...items.map((item) => h('span', { class: `item tone-${item.tone}` },
      h('span', { class: 'dot' }), item.text)),
    h('span', { class: 'spacer' }),
    address && h('span', { class: 'item muted', title: '다른 컴퓨터가 연결할 주소' }, '주소 ', h('code', { text: address })),
    toggle);
}

function renderBanners() {
  const banners = $('banners');
  banners.replaceChildren();

  const notice = localError || snap.notice;
  if (notice) {
    banners.append(
      h('div', { class: 'banner' },
        h('span', { class: 'text', text: notice }),
        h('button', { text: '닫기', on: { click: dismissNotice } })),
    );
  }

  const missing = snap.permissions.filter((permission) => !permission.granted).map((p) => p.name);
  if (missing.length > 0) {
    banners.append(
      h('div', { class: 'banner warn' },
        h('span', {
          class: 'text',
          text: `키보드와 마우스를 다루려면 시스템 설정의 개인정보 보호 및 보안에서 이 앱에 권한이 필요합니다: ${missing.join(', ')}`,
        }),
        h('button', { text: '권한 요청', on: { click: () => act('request_permissions') } }),
        h('button', { text: '시스템 설정 열기', on: { click: () => act('open_privacy_settings') } })),
    );
  }
}

function dismissNotice() {
  localError = null;
  act('dismiss_notice');
}

function renderTabs() {
  const tabs = $('tabs');
  tabs.replaceChildren(
    ...tabsFor().map(([id, label]) =>
      h('button', {
        type: 'button',
        class: id === tab ? 'on' : '',
        text: label,
        on: { click: () => { if (id === 'settings' && tab !== 'settings') previousTab = tab; tab = id; builtView = null; render(); } },
      })),
  );
}

function renderView() {
  const view = $('view');
  // The layout fills the window; the other tabs sit in a padded page.
  view.classList.toggle('full', (tab === 'layout' && snap.mode === 'share') || tab === 'settings');
  switch (tab) {
    case 'layout': return snap.mode === 'share' ? layoutView(view) : receivingLayoutView(view);
    case 'devices': return devicesView(view);
    case 'activity': return activityView(view);
    case 'settings': return settingsView(view);
    default: return undefined;
  }
}

// ---- the layout editor ----

/** How close, in canvas pixels, a screen has to be to another's edge to jump onto it. */
const SNAP_PX = 14;

/** How far a computer is drawn inside its edges, so that the line where it meets a neighbor has room. */
const EDGE_GAP = 5;

/** How thick, in canvas pixels, the line where computers touch is. */
const LINK_PX = 3;

/** The canvas as last drawn: where the desktop is on it, for turning pointer positions back. */
let canvasState = null;

/** How much of the canvas edges the floating panels cover, so that fitting keeps clear of them. */
const FLOATING_INSETS = { left: 20, right: 310, top: 20, bottom: 60 };

/**
 * The view the person chose by zooming or panning, or null to fit everything. It is kept
 * between redraws, so the layout does not jump back each time something changes.
 */
let viewOverride = null;

/** How far the layout can be zoomed, in canvas pixels per desktop pixel. */
const MIN_SCALE = 0.004;
const MAX_SCALE = 0.6;

function layoutView(view) {
  view.replaceChildren(buildLayout());
  builtView = 'layout';
  const canvas = view.querySelector('.canvas');
  watchCanvas(canvas);
  paintCanvas(canvas);
}

function buildLayout() {
  return h('section', { class: 'layout' },
    h('div', { class: 'canvas' }),
    h('div', { class: 'zoom float', id: 'zoom-controls' },
      h('button', { type: 'button', title: '축소', text: '−', on: { click: () => zoomBy(1 / 1.25) } }),
      h('button', { type: 'button', class: 'level', id: 'zoom-level', title: '전체가 보이게 맞추기', on: { click: fitAll } }),
      h('button', { type: 'button', title: '확대', text: '+', on: { click: () => zoomBy(1.25) } })),
    h('div', { class: 'side' }, buildDetail()));
}

function boundsSize(displays) {
  const all = boundsOf(displays);
  return [all.width, all.height];
}

// ---- icons ----
//
// Fixed drawings, written here and nowhere else, so they are safe to put in the page as markup.

const ICONS = {
  warn: '<circle cx="12" cy="12" r="11" fill="currentColor"/><rect x="10.7" y="5.2" width="2.6" height="8.6" rx="1.3" class="cut"/><circle cx="12" cy="17.9" r="1.6" class="cut"/>',
  mac: '<path fill="currentColor" d="M16.365 1.43c0 1.14-.493 2.27-1.177 3.08-.744.9-1.99 1.57-2.987 1.57-.12 0-.23-.02-.3-.03-.01-.06-.04-.22-.04-.39 0-1.15.572-2.27 1.206-2.98.804-.94 2.142-1.64 3.248-1.68.03.13.05.28.05.43zm4.565 15.71c-.03.07-.463 1.58-1.518 3.12-.945 1.34-1.94 2.71-3.43 2.71-1.517 0-1.9-.88-3.63-.88-1.698 0-2.302.91-3.67.91-1.377 0-2.332-1.26-3.428-2.8-1.287-1.82-2.323-4.63-2.323-7.28 0-4.28 2.797-6.55 5.552-6.55 1.448 0 2.675.95 3.6.95.865 0 2.222-1.01 3.902-1.01.613 0 2.886.06 4.374 2.19-.13.09-2.383 1.37-2.383 4.19 0 3.26 2.854 4.42 2.955 4.45z"/>',
  windows: '<path fill="currentColor" d="M0 3.449L9.75 2.1v9.451H0m10.949-9.602L24 0v11.4H10.949M0 12.6h9.75v9.451L0 20.699M10.949 12.6H24V24l-12.9-1.801"/>',
  linux: '<ellipse cx="12" cy="14.8" rx="6.6" ry="8" fill="currentColor"/><circle cx="12" cy="6.6" r="4.3" fill="currentColor"/><ellipse cx="7.6" cy="22" rx="3.2" ry="1.7" fill="currentColor"/><ellipse cx="16.4" cy="22" rx="3.2" ry="1.7" fill="currentColor"/><ellipse cx="12" cy="16.2" rx="3.9" ry="5.6" class="cut"/><circle cx="10.4" cy="5.8" r="0.9" class="cut"/><circle cx="13.6" cy="5.8" r="0.9" class="cut"/><path d="M10.3 7.6l1.7 1.5 1.7-1.5-1.7-.7z" class="beak"/>',
  play: '<path d="M7 4.5v15l12-7.5z" fill="currentColor"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor"/>',
  close: '<path d="M6 6l12 12M18 6L6 18" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/>',
  plus: '<path d="M12 5v14M5 12h14" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"/>',
  remove: '<circle cx="12" cy="12" r="9.5" fill="none" stroke="currentColor" stroke-width="2"/><path d="M7.5 12h9" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"/>',
  other: '<rect x="2" y="4" width="20" height="13" rx="2" fill="currentColor"/><rect x="8" y="19" width="8" height="2" rx="1" fill="currentColor"/>',
};

/** A tooltip for an icon. */
function tip(text) {
  const title = document.createElementNS('http://www.w3.org/2000/svg', 'title');
  title.textContent = text;
  return title;
}

/** An icon from `ICONS`, drawn at `size` pixels. */
function icon(kind, size, className) {
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('width', String(size));
  svg.setAttribute('height', String(size));
  svg.setAttribute('class', `icon ${className}`);
  svg.setAttribute('aria-hidden', 'true');
  svg.innerHTML = ICONS[kind] || ICONS.other;
  return svg;
}

const OS_NAMES = { mac: 'macOS', windows: 'Windows', linux: 'Linux', other: '알 수 없는 운영체제' };

/**
 * Builds a computer as one rectangle, with no borders anywhere. Unselected it shows only its
 * name. Selected (`expanded`) it shows its monitors where they are, each with its own facts:
 * resolution, magnification and whether it is the main one. `at` is where its corner goes, in
 * pixels, on whatever the block is placed on.
 *
 * `options`: `tone`, `extra` (more classes), `expanded`, `picked` (the number of the monitor
 * the person pressed), `warn` (the cursor can not get here, so an exclamation mark sits in the
 * top left corner). The system's icon sits in the bottom right corner.
 */
function buildGroup(screen, view, at, options) {
  const { tone, extra = '', expanded = false, picked = null, warn = false } = options;
  // The block is a little smaller on every side, so that a line where it meets a neighbor
  // has some room on both sides.
  const multi = screen.displays.length > 1 && expanded;
  const displays = expanded ? screen.displays : [{ x: 0, y: 0, width: screen.width, height: screen.height }];
  const width = Math.max(4, screen.width * view.scale - EDGE_GAP * 2);
  const height = Math.max(4, screen.height * view.scale - EDGE_GAP * 2);
  const group = h('div', { class: `group ${tone} ${multi ? 'multi' : ''} ${extra}`.replace(/ +/g, ' ').trim() });
  Object.assign(group.style, {
    left: `${at.x + EDGE_GAP}px`,
    top: `${at.y + EDGE_GAP}px`,
    width: `${width}px`,
    height: `${height}px`,
  });
  const badge = Math.max(11, Math.min(20, Math.min(width, height) / 3.2));

  // With several monitors, each sits inside the computer's rectangle with a little room
  // around it; with one, it is the rectangle.
  const gap = multi ? 1.5 : 0;
  displays.forEach((display, index) => {
    const monitor = h('div', { class: `monitor${index === 0 ? ' main' : ''}${expanded && picked === index ? ' picked' : ''}` });
    Object.assign(monitor.style, {
      left: `${display.x * view.scale - EDGE_GAP + gap}px`,
      top: `${display.y * view.scale - EDGE_GAP + gap}px`,
      width: `${display.width * view.scale - gap * 2}px`,
      height: `${display.height * view.scale - gap * 2}px`,
      // Small monitors get small type, so that what is on them still fits.
      fontSize: `${Math.max(10, Math.min(14, (display.width * view.scale) / 9))}px`,
    });
    if (expanded) {
      monitor.dataset.index = String(index);
      // Room for the exclamation mark in the corner.
      if (warn && index === 0) monitor.style.paddingLeft = `${badge + 10}px`;
      const facts = monitorFacts(display, (screen.scales || [])[index], index);
      if (facts.main) monitor.append(h('div', { class: 'tag', text: '주 모니터' }));
      monitor.append(
        h('div', { class: 'res', text: facts.resolution }),
        h('div', { class: 'sub', text: `배율 ${facts.scale}` }));
    } else {
      const name = h('div', { class: 'name', text: screen.name, title: screen.name });
      // Room for the exclamation mark, which sits where the name starts.
      if (warn) name.style.paddingLeft = `${badge + 4}px`;
      monitor.append(h('span', { class: 'corner' }), name);
    }
    group.append(monitor);
  });

  if (warn) {
    const mark = icon('warn', badge, 'badge warn');
    mark.style.left = '6px';
    mark.style.top = '6px';
    mark.append(tip('맞닿은 화면이 없어서 마우스가 넘어갈 수 없어요'));
    group.append(mark);
  }
  const system = icon(screen.os || 'other', badge, 'badge os');
  system.style.right = '6px';
  system.style.bottom = '6px';
  system.append(tip(OS_NAMES[screen.os] || OS_NAMES.other));
  group.append(system);
  return group;
}

/** The view to draw with: what the person zoomed to, or everything fitted to the canvas. */
function currentView(canvas) {
  const spare = snap.devices
    .filter((device) => !isPlaced(snap.screens, device.name))
    .map((device) => {
      const [w, h2] = boundsSize(device.displays);
      return { width: w, height: h2 };
    });
  const fit = fitView(snap.screens, canvas.clientWidth, canvas.clientHeight, spare, FLOATING_INSETS);
  return { fit, view: viewOverride || fit };
}

/** Draws the screens on the canvas, scaled to fit. Needs the canvas to be on the page. */
function paintCanvas(canvas) {
  const width = canvas.clientWidth;
  const height = canvas.clientHeight;
  if (width === 0 || height === 0) return;

  const active = snap.server ? snap.server.active_screen : null;
  const lost = new Set(unreachable(snap.screens, snap.this_computer));
  const { fit, view } = currentView(canvas);
  canvasState = { canvas, view, fit };
  const level = document.getElementById('zoom-level');
  if (level) level.textContent = `${Math.round((view.scale / fit.scale) * 100)}%`;

  for (const edge of contactSegments(snap.screens)) canvas.append(linkBar(view, edge, ''));

  for (const screen of snap.screens) {
    const tone = toneOf(screen);
    const warn = lost.has(screen.name);
    const expanded = sole() === screen.name;
    const together = selection.size > 1 && selection.has(screen.name);
    const element = buildGroup(screen, view, toCanvas(view, screen.x, screen.y), {
      tone,
      extra: [active === screen.name ? 'has-cursor' : '', expanded ? 'selected' : '', together ? 'in-selection' : ''].join(' '),
      expanded,
      picked: expanded ? pickedMonitor : null,
      warn,
    });
    element.dataset.name = screen.name;
    element.addEventListener('pointerdown', (event) => beginDrag(event, element, screen.name));
    canvas.append(element);
  }
}

/** How a computer is shown: this one in blue, others green when connected and grey when not. */
function toneOf(screen) {
  if (screen.local) return 'accent';
  const device = snap.devices.find((candidate) => candidate.name === screen.name);
  return device && device.online ? 'good' : 'off';
}

/** Draws the canvas again without touching the rest of the page, for zooming and panning. */
function repaintCanvas() {
  if (!canvasState) return;
  const { canvas } = canvasState;
  canvas.replaceChildren();
  paintCanvas(canvas);
}

// ---- zooming and panning ----

/** Zooms about the middle of the canvas by `factor`, as the + and − buttons do. */
function zoomBy(factor) {
  if (!canvasState) return;
  const { canvas, view } = canvasState;
  viewOverride = zoomAt(view, factor, canvas.clientWidth / 2, canvas.clientHeight / 2, MIN_SCALE, MAX_SCALE);
  repaintCanvas();
}

/** Goes back to showing everything. */
function fitAll() {
  viewOverride = null;
  repaintCanvas();
}

/**
 * What the canvas itself responds to: the wheel zooms about the pointer, and dragging empty
 * canvas moves the view. A press on empty canvas that goes nowhere lets go of the selection.
 */
function watchCanvas(canvas) {
  canvas.addEventListener('wheel', (event) => {
    event.preventDefault();
    if (!canvasState) return;
    const area = canvas.getBoundingClientRect();
    // A pinch on a trackpad arrives as a wheel with Ctrl held, in smaller steps.
    const rate = event.ctrlKey ? 0.01 : 0.0018;
    const factor = Math.exp(-event.deltaY * rate);
    viewOverride = zoomAt(
      canvasState.view, factor, event.clientX - area.left, event.clientY - area.top, MIN_SCALE, MAX_SCALE,
    );
    repaintCanvas();
  }, { passive: false });

  canvas.addEventListener('contextmenu', (event) => event.preventDefault());
  canvas.addEventListener('pointerdown', (event) => {
    if (event.target !== canvas) return;
    // Space held, or the middle or right button, drags the view; the plain button draws a
    // rectangle that selects what it touches.
    const panning = event.button === 1 || event.button === 2 || (event.button === 0 && spaceDown);
    if (!panning && event.button !== 0) return;
    event.preventDefault();
    if (panning) startPanning(canvas, event);
    else startSelecting(canvas, event);
  });
}

/** Moves the view with the pointer until it is let go. */
function startPanning(canvas, press) {
  let last = { x: press.clientX, y: press.clientY };
  canvas.classList.add('panning');
  const onMove = (move) => {
    viewOverride = panBy(canvasState.view, move.clientX - last.x, move.clientY - last.y);
    last = { x: move.clientX, y: move.clientY };
    repaintCanvas();
  };
  const onUp = () => {
    window.removeEventListener('pointermove', onMove);
    window.removeEventListener('pointerup', onUp);
    window.removeEventListener('pointercancel', onUp);
    canvas.classList.remove('panning');
  };
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', onUp);
  window.addEventListener('pointercancel', onUp);
}

/**
 * Draws a rectangle from where the pointer went down to where it is, and selects the computers
 * it touches when let go (with Shift, in addition to those already selected). Pressed and let
 * go in place, it lets go of the selection.
 */
function startSelecting(canvas, press) {
  const area = canvas.getBoundingClientRect();
  const start = { x: press.clientX - area.left, y: press.clientY - area.top };
  const additive = press.shiftKey || press.metaKey || press.ctrlKey;
  const band = h('div', { class: 'marquee' });
  let moved = false;
  let hits = [];

  const onMove = (move) => {
    const here = { x: move.clientX - area.left, y: move.clientY - area.top };
    if (!moved && Math.hypot(here.x - start.x, here.y - start.y) < 4) return;
    moved = true;
    if (!band.isConnected) canvas.append(band);
    const rect = {
      x: Math.min(start.x, here.x),
      y: Math.min(start.y, here.y),
      width: Math.abs(here.x - start.x),
      height: Math.abs(here.y - start.y),
    };
    Object.assign(band.style, { left: `${rect.x}px`, top: `${rect.y}px`, width: `${rect.width}px`, height: `${rect.height}px` });
    const { view } = canvasState;
    hits = snap.screens.filter((screen) => {
      const at = toCanvas(view, screen.x, screen.y);
      return overlapRegions([rect], [{ x: at.x, y: at.y, width: screen.width * view.scale, height: screen.height * view.scale }]).length > 0;
    }).map((screen) => screen.name);
    for (const element of canvas.querySelectorAll('.group')) {
      element.classList.toggle('marquee-hit', hits.includes(element.dataset.name));
    }
  };
  const onUp = () => {
    window.removeEventListener('pointermove', onMove);
    window.removeEventListener('pointerup', onUp);
    window.removeEventListener('pointercancel', onUp);
    band.remove();
    if (!moved) {
      if (selection.size > 0 && !additive) deselect();
      return;
    }
    selection = new Set(additive ? [...selection, ...hits] : hits);
    pickedMonitor = null;
    render();
  };
  window.addEventListener('pointermove', onMove);
  window.addEventListener('pointerup', onUp);
  window.addEventListener('pointercancel', onUp);
}

/**
 * A line over a stretch of the edge where two computers touch: green where the cursor can
 * pass, red where it cannot. Only the two ends of the whole contact are rounded; where a green
 * stretch meets a red one the line goes straight on. `extra` adds a class, such as `preview`.
 */
function linkBar(view, edge, extra) {
  const from = toCanvas(view, edge.x1, edge.y1);
  const to = toCanvas(view, edge.x2, edge.y2);
  const vertical = edge.x1 === edge.x2;
  const bar = h('div', { class: `link ${edge.ok ? 'ok' : 'blocked'} ${extra}`.trim() });
  const round = (on) => (on ? `${LINK_PX / 2}px` : '0');

  // The computers are drawn a little inside their edges, so the whole contact is that much
  // shorter at each of its ends, to be exactly as long as the rectangles that touch. Where one
  // stretch meets the next, there is nothing to take off.
  const start = (vertical ? from.y : from.x) + (edge.capStart ? EDGE_GAP : 0);
  const end = (vertical ? to.y : to.x) - (edge.capEnd ? EDGE_GAP : 0);
  const length = Math.max(0, end - start);

  Object.assign(bar.style, {
    left: `${vertical ? from.x - LINK_PX / 2 : start}px`,
    top: `${vertical ? start : from.y - LINK_PX / 2}px`,
    width: `${vertical ? LINK_PX : length}px`,
    height: `${vertical ? length : LINK_PX}px`,
    // Corners in the order top left, top right, bottom right, bottom left.
    borderRadius: vertical
      ? `${round(edge.capStart)} ${round(edge.capStart)} ${round(edge.capEnd)} ${round(edge.capEnd)}`
      : `${round(edge.capStart)} ${round(edge.capEnd)} ${round(edge.capEnd)} ${round(edge.capStart)}`,
  });
  return bar;
}

/** The system's icon, small, to sit in a line of text. */
function osMark(os, size = 15) {
  const mark = icon(os || 'other', size, 'inline');
  mark.append(tip(OS_NAMES[os] || OS_NAMES.other));
  return mark;
}

/** A round button with only an icon; `label` is what it does, for the tooltip and for readers. */
function iconButton(kind, label, onClick) {
  const button = h('button', { type: 'button', class: 'icon-btn', title: label, 'aria-label': label, on: { click: onClick } });
  button.append(icon(kind, 16, 'inline'));
  return button;
}

/**
 * Presses a computer on the layout. On its own it becomes the one selected, and its details
 * show. With Shift (or Ctrl, or Cmd) it is added to the selection or taken out of it.
 */
function pick(name, monitor = null, additive = false) {
  if (additive) {
    if (selection.has(name)) selection.delete(name);
    else selection.add(name);
    pickedMonitor = null;
  } else {
    selection = new Set([name]);
    pickedMonitor = monitor;
  }
  render();
}

function deselect() {
  selection = new Set();
  pickedMonitor = null;
  render();
}

/** The card when several computers are selected: who they are, and that they move together. */
function buildGroupDetail() {
  const chosen = snap.screens.filter((screen) => selection.has(screen.name));
  return h('aside', { class: 'detail float' },
    h('div', { class: 'head' },
      h('h3', { text: `컴퓨터 ${chosen.length}대 선택됨` }),
      iconButton('close', '선택 풀기', deselect)),
    h('p', { class: 'muted small', text: '끌면 한꺼번에 움직입니다. Shift를 누르고 누르면 하나씩 더하거나 뺄 수 있습니다.' }),
    h('div', { class: 'monitors' }, chosen.map((screen) => h('div', { class: 'monitor-row' },
      h('div', { class: 'title' }, osMark(screen.os, 15), h('span', { text: screen.name }))))));
}

/**
 * The floating card for the selected computer: how it is connected, its system, and every
 * monitor with its resolution and magnification. The monitor that was pressed is marked.
 */
function buildDetail() {
  if (selection.size > 1) return buildGroupDetail();
  const screen = snap.screens.find((candidate) => candidate.name === sole());
  if (!screen) return null;
  const device = snap.devices.find((candidate) => candidate.name === screen.name);
  const active = snap.server ? snap.server.active_screen : null;

  const facts = [];
  const fact = (label, value) => facts.push(h('div', { class: 'fact' },
    h('span', { class: 'label', text: label }), h('span', { class: 'value', text: value })));
  if (screen.local) {
    fact('상태', '이 컴퓨터');
    const address = shareAddress(snap);
    if (address) fact('주소', address);
    fact('지문', snap.fingerprint_short);
  } else if (device) {
    fact('상태', device.online ? '연결됨' : '오프라인');
    if (device.address) fact('주소', device.address);
    fact('지문', device.fingerprint_short);
  }
  fact('운영체제', OS_NAMES[screen.os] || OS_NAMES.other);
  if (active === screen.name) fact('커서', '여기에 있음');

  const monitors = screen.displays.map((display, index) => {
    const info = monitorFacts(display, (screen.scales || [])[index], index);
    return h('div', {
      class: `monitor-row${pickedMonitor === index ? ' picked' : ''}`,
      on: { click: () => { pickedMonitor = index; render(); } },
    },
    h('div', { class: 'title' }, `모니터 ${index + 1}`, info.main && h('span', { class: 'tag', text: '주 모니터' })),
    h('div', { class: 'fact' }, h('span', { class: 'label', text: '해상도' }), h('span', { class: 'value', text: info.resolution })),
    h('div', { class: 'fact' }, h('span', { class: 'label', text: '배율' }), h('span', { class: 'value', text: info.scale })));
  });

  return h('aside', { class: 'detail float' },
    h('div', { class: 'head' },
      osMark(screen.os, 18),
      h('h3', { text: screen.name, title: screen.name }),
      screen.local ? null : iconButton('remove', '배치에서 빼기', () => {
        deselect();
        act('remove_from_layout', { device: screen.name });
      }),
      iconButton('close', '닫기', deselect)),
    h('div', { class: 'facts' }, facts),
    h('div', { class: 'monitors' }, monitors));
}

// ---- dragging screens ----

/** How far, in pixels, the pointer has to travel before a press counts as a drag, not a click. */
const DRAG_START_PX = 5;

function endDrag() {
  if (drag) {
    for (const ghost of drag.ghosts) ghost.remove();
    clearPreview();
    for (const element of document.querySelectorAll('.group.dragging')) element.classList.remove('dragging');
    document.body.classList.remove('is-dragging');
    window.removeEventListener('pointermove', onDragMove);
    window.removeEventListener('pointerup', onDragEnd);
    window.removeEventListener('pointercancel', onDragCancel);
    window.removeEventListener('keydown', onDragKey);
  }
  drag = null;
  if (renderPending) {
    renderPending = false;
    render();
  }
}

function clearPreview() {
  for (const element of document.querySelectorAll('.preview')) element.remove();
}

/** What is needed to draw and move a computer while it is carried. */
function carried(screen, anchor, lost) {
  return {
    name: screen.name,
    local: screen.local,
    width: screen.width,
    height: screen.height,
    displays: screen.displays,
    scales: screen.scales,
    os: screen.os,
    tone: toneOf(screen),
    warn: lost.has(screen.name),
    // Where it is in relation to the computer that was pressed.
    dx: screen.x - anchor.x,
    dy: screen.y - anchor.y,
  };
}

/**
 * A press on a computer on the layout. Pressed and let go without moving, it is a click, and
 * selects the computer (with Shift, adds it to the selection). Moved, it is picked up: if it is
 * one of several selected computers they are all picked up together, otherwise just it. They
 * follow the pointer as ghosts, jump onto the edges of the others when they come close, and go
 * where they are dropped. Where they would sit on top of another computer, that area turns red.
 */
function beginDrag(event, source, name) {
  if (event.button !== 0 || drag || !canvasState || spaceDown) return;
  event.preventDefault();

  const { view } = canvasState;
  const anchor = snap.screens.find((screen) => screen.name === name);
  if (!anchor) return;
  const lost = new Set(unreachable(snap.screens, snap.this_computer));
  const chosen = selection.size > 1 && selection.has(name)
    ? snap.screens.filter((screen) => selection.has(screen.name))
    : [anchor];
  const members = chosen.map((screen) => carried(screen, anchor, lost));
  // The rectangle round all of them, from the pressed computer's corner.
  const all = boundsOf(members.map((m) => ({ x: m.dx, y: m.dy, width: m.width, height: m.height })));
  const pressed = source.getBoundingClientRect();

  drag = {
    name,
    members,
    union: { x: all.x, y: all.y, width: all.width, height: all.height },
    ghosts: [],
    active: false,
    additive: event.shiftKey || event.metaKey || event.ctrlKey,
    clickedMonitor: (event.target.closest ? event.target.closest('.monitor[data-index]') : null)?.dataset.index ?? null,
    pressedAt: { x: event.clientX, y: event.clientY },
    startedAt: Date.now(),
    // Where in the pressed computer the pointer holds it.
    grabX: event.clientX - pressed.left,
    grabY: event.clientY - pressed.top,
    target: null,
  };
  if (drag.clickedMonitor !== null) drag.clickedMonitor = Number(drag.clickedMonitor);
  window.addEventListener('pointermove', onDragMove);
  window.addEventListener('pointerup', onDragEnd);
  window.addEventListener('pointercancel', onDragCancel);
  window.addEventListener('keydown', onDragKey);
}

/** Picks the computers up, once the pointer has moved far enough. */
function startCarrying() {
  const { view } = canvasState;
  drag.active = true;
  for (const member of drag.members) {
    const ghost = buildGroup({ ...member }, view, { x: -EDGE_GAP, y: -EDGE_GAP }, {
      tone: member.tone,
      extra: 'ghost',
      expanded: drag.members.length === 1 && sole() === member.name,
      warn: member.warn,
    });
    document.body.append(ghost);
    drag.ghosts.push(ghost);
    const element = canvasState.canvas.querySelector(`.group[data-name="${CSS.escape(member.name)}"]`);
    if (element) element.classList.add('dragging');
  }
  document.body.classList.add('is-dragging');
}

/** Works out where the dragged computers would land for a pointer at this position. */
function dropTarget(clientX, clientY) {
  const { canvas, view } = canvasState;
  const area = canvas.getBoundingClientRect();
  const under = document.elementFromPoint(clientX, clientY);

  // The floating cards are not somewhere to drop.
  if (under && under.closest('.side, .zoom')) return { kind: 'nowhere' };
  if (clientX < area.left || clientX > area.right || clientY < area.top || clientY > area.bottom) {
    return { kind: 'nowhere' };
  }

  const names = new Set(drag.members.map((member) => member.name));
  const others = snap.screens.filter((screen) => !names.has(screen.name));
  const pos = toDesktop(view, clientX - area.left - drag.grabX, clientY - area.top - drag.grabY);
  // They snap as one block, by the rectangle round all of them.
  const { union } = drag;
  const block = { displays: [{ x: 0, y: 0, width: union.width, height: union.height }], width: union.width, height: union.height };
  const settled = snapPosition(block, Math.round(pos.x) + union.x, Math.round(pos.y) + union.y, others, SNAP_PX / view.scale);
  const x = settled.x - union.x;
  const y = settled.y - union.y;

  const blockers = [];
  const clash = [];
  for (const member of drag.members) {
    for (const other of blockersAt(member, x + member.dx, y + member.dy, others)) {
      if (!blockers.includes(other)) blockers.push(other);
    }
    clash.push(...overlapRegions([boxOf(member, x, y)], others.map((o) => box(o))));
  }
  return { kind: 'canvas', x, y, blockers, clash };
}

/** The rectangle of a carried computer when the pressed one is at (`x`, `y`). */
function boxOf(member, x, y) {
  return box(member, x + member.dx, y + member.dy);
}

/** Draws, on the canvas, where the computers would touch the others and where they would collide. */
function drawPreview(target) {
  const { canvas, view } = canvasState;
  clearPreview();
  if (target.kind !== 'canvas') return;

  const names = new Set(drag.members.map((member) => member.name));
  const ghosts = drag.members.map((member) => ({ ...member, x: target.x + member.dx, y: target.y + member.dy }));
  const others = snap.screens.filter((screen) => !names.has(screen.name));
  for (const edge of contactSegments([...ghosts, ...others])) {
    // Only where a carried computer meets one that stays.
    if (names.has(edge.a) !== names.has(edge.b)) canvas.append(linkBar(view, edge, 'preview'));
  }
  // Over the ghosts, which are themselves above the canvas, so these sit on the page, not in it.
  const area = canvas.getBoundingClientRect();
  for (const region of target.clash) {
    const spot = toCanvas(view, region.x, region.y);
    const clash = h('div', { class: 'clash preview' });
    Object.assign(clash.style, {
      left: `${area.left + spot.x}px`,
      top: `${area.top + spot.y}px`,
      width: `${region.width * view.scale}px`,
      height: `${region.height * view.scale}px`,
    });
    document.body.append(clash);
  }
}

function onDragMove(event) {
  if (!drag) return;
  if (!drag.active) {
    if (Math.hypot(event.clientX - drag.pressedAt.x, event.clientY - drag.pressedAt.y) < DRAG_START_PX) return;
    startCarrying();
  }
  const { canvas, view } = canvasState;
  const target = dropTarget(event.clientX, event.clientY);
  drag.target = target;

  // Where the pressed computer's ghost goes: where the pointer holds it, or on the canvas spot
  // it would settle on.
  let left = event.clientX - drag.grabX;
  let top = event.clientY - drag.grabY;
  if (target.kind === 'canvas') {
    const area = canvas.getBoundingClientRect();
    const spot = toCanvas(view, target.x, target.y);
    left = area.left + spot.x;
    top = area.top + spot.y;
  }
  drag.members.forEach((member, index) => {
    const ghost = drag.ghosts[index];
    ghost.style.left = `${left + member.dx * view.scale + EDGE_GAP}px`;
    ghost.style.top = `${top + member.dy * view.scale + EDGE_GAP}px`;
    ghost.classList.toggle('invalid', target.kind === 'canvas' && target.blockers.length > 0);
    ghost.classList.toggle('floating', target.kind !== 'canvas');
  });
  drawPreview(target);
}

function onDragEnd(event) {
  if (!drag) return;
  if (!drag.active) {
    // Pressed and let go in place: a click. Dragging never changes what is selected.
    const { name, clickedMonitor, additive } = drag;
    endDrag();
    pick(name, clickedMonitor, additive);
    return;
  }
  onDragMove(event);
  const { target, name, members } = drag;
  endDrag();

  if (target.kind !== 'canvas') return;
  if (target.blockers.length > 0) {
    localError = `‘${target.blockers[0].name}’ 화면과 겹치게 놓을 수 없습니다.`;
    render();
    return;
  }
  const current = snap.screens.find((screen) => screen.name === name);
  const [dx, dy] = [target.x - current.x, target.y - current.y];
  if (dx === 0 && dy === 0) return;

  if (members.length > 1) {
    act('move_group', {
      devices: members.filter((member) => !member.local).map((member) => member.name),
      local: members.some((member) => member.local),
      dx,
      dy,
    });
  } else if (current.local) {
    act('move_this_computer', { x: target.x, y: target.y });
  } else {
    act('place_device', { device: name, x: target.x, y: target.y });
  }
}

function onDragCancel() {
  endDrag();
}

function onDragKey(event) {
  if (event.key === 'Escape') endDrag();
}

// ---- the other tabs ----

/** In receive mode the layout belongs to the computer that shares, so there is nothing to place. */
function receivingLayoutView(view) {
  view.replaceChildren(h('section', { class: 'stack' },
    h('h2', { text: '레이아웃' }),
    h('p', {
      class: 'muted',
      text: '이 컴퓨터는 다른 컴퓨터의 키보드와 마우스를 받는 쪽입니다. 화면 배치는 공유하는 컴퓨터에서 합니다. 이 컴퓨터를 공유하는 쪽으로 쓰려면 ‘설정’에서 바꾸세요.',
    })));
  builtView = null;
}

// ---- settings ----
//
// Like the settings of a chat program: the page takes over the whole window, with the
// categories down the left and the chosen one on the right, and Esc (or the round button in the
// corner) goes back to where the person was.

let settingsPage = 'account';
let previousTab = 'layout';

/** The categories, in groups: `[group title, [[id, label], ...]]`. */
function settingsCategories() {
  return [
    ['이 컴퓨터', [['account', '내 정보']]],
    ['공유', [['mode', '사용 방식'], ...(snap.mode === 'receive' ? [['connection', '연결할 컴퓨터']] : [])]],
    ['앱', [['permissions', '권한'], ['help', '도움말']]],
  ];
}

function closeSettings() {
  tab = previousTab && previousTab !== 'settings' ? previousTab : 'layout';
  builtView = null;
  render();
}

function settingsView(view) {
  if (settingsPage === 'connection' && snap.mode !== 'receive') settingsPage = 'mode';
  if (builtView !== 'settings' || builtSettingsMode !== `${snap.mode}/${settingsPage}`) {
    view.replaceChildren(h('div', { class: 'settings-page' },
      h('nav', { class: 'settings-nav' },
        ...settingsCategories().flatMap(([title, items]) => [
          h('div', { class: 'group-title', text: title }),
          ...items.map(([id, label]) => h('button', {
            type: 'button',
            class: `item${id === settingsPage ? ' on' : ''}`,
            text: label,
            on: { click: () => { settingsPage = id; builtView = null; render(); } },
          })),
        ])),
      h('div', { class: 'settings-content' },
        h('div', { class: 'settings-body' }, settingsContent()),
        h('button', {
          type: 'button',
          class: 'settings-close',
          title: '닫기 (Esc)',
          'aria-label': '설정 닫기',
          on: { click: closeSettings },
        }, icon('close', 18, 'inline'), h('span', { text: 'ESC' })))));
    builtView = 'settings';
    builtSettingsMode = `${snap.mode}/${settingsPage}`;
  }

  // What changes while the page is open.
  for (const button of view.querySelectorAll('#mode button')) {
    button.classList.toggle('on', button.id === `mode-${snap.mode}`);
    button.disabled = snap.running;
  }
  const mode = document.getElementById('mode');
  if (mode) mode.classList.toggle('locked', snap.running);
  const note = document.getElementById('mode-note');
  if (note) {
    note.textContent = snap.running
      ? '실행 중에는 바꿀 수 없습니다. 아래 상태 표시줄에서 먼저 중지하세요.'
      : snap.mode === 'share'
        ? '이 컴퓨터의 키보드와 마우스로 다른 컴퓨터를 조작합니다.'
        : '다른 컴퓨터의 키보드와 마우스로 이 컴퓨터를 조작합니다.';
  }
  const input = document.getElementById('server-address');
  if (input) {
    // The address box is left alone while the person is typing in it.
    input.placeholder = `예: 192.168.0.12  (포트 ${snap.server_port}는 생략 가능)`;
    input.disabled = snap.running;
    if (document.activeElement !== input && input.value !== snap.server_address) input.value = snap.server_address;
    $('connection-status').replaceChildren(...connectionStatus());
  }
  const permissions = document.getElementById('permissions');
  if (permissions) permissions.replaceChildren(...permissionRows());
}

function permissionRows() {
  if (snap.permissions.length === 0) {
    return [h('p', { class: 'muted', text: '이 컴퓨터에서는 따로 허용할 권한이 없습니다.' })];
  }
  const missing = snap.permissions.some((permission) => !permission.granted);
  return [
    ...snap.permissions.map((permission) => h('div', { class: 'row perm' },
      h('span', { class: `dot ${permission.granted ? 'good' : 'warn'}` }),
      h('span', { class: 'name', text: permission.name }),
      h('span', { class: 'muted', text: permission.granted ? '허용됨' : '허용 필요' }))),
    missing
      ? h('div', { class: 'actions' },
        h('button', { type: 'button', class: 'secondary', text: '권한 요청', on: { click: () => act('request_permissions') } }),
        h('button', { type: 'button', class: 'secondary', text: '시스템 설정 열기', on: { click: () => act('open_privacy_settings') } }))
      : null,
  ].filter(Boolean);
}

/** The page chosen in the list on the left. */
function settingsContent() {
  const local = snap.screens.find((screen) => screen.local);
  switch (settingsPage) {
    case 'mode': {
      const modeButton = (mode, label) => h('button', {
        type: 'button',
        id: `mode-${mode}`,
        text: label,
        on: {
          click: () => {
            if (!snap || snap.running || snap.mode === mode) return;
            builtView = null;
            act('set_mode', { mode });
          },
        },
      });
      return [
        h('h2', { text: '사용 방식' }),
        h('div', { id: 'mode', class: 'segmented', role: 'group', 'aria-label': '사용 방식' },
          modeButton('share', '이 컴퓨터 공유'), modeButton('receive', '다른 컴퓨터에서 받기')),
        h('p', { id: 'mode-note', class: 'muted' }),
      ];
    }
    case 'connection':
      return [
        h('h2', { text: '연결할 컴퓨터' }),
        h('p', {
          class: 'muted',
          text: '다른 컴퓨터에서 ‘이 컴퓨터 공유’로 시작하면 그 컴퓨터가 알려 주는 주소를 아래에 입력하세요. 처음이라면 먼저 ‘기기’ 탭에서 그 컴퓨터와 페어링해야 합니다.',
        }),
        h('div', { class: 'field' },
          h('label', { for: 'server-address', text: '주소' }),
          h('input', {
            type: 'text',
            id: 'server-address',
            value: snap.server_address,
            autocomplete: 'off',
            spellcheck: false,
            on: { input: (event) => api('set_server_address', { address: event.target.value }).catch(() => {}) },
          })),
        h('div', { id: 'connection-status' }),
      ];
    case 'permissions':
      return [
        h('h2', { text: '권한' }),
        h('p', { class: 'muted', text: '키보드와 마우스를 읽고 보내려면 운영체제의 허용이 필요합니다.' }),
        h('div', { id: 'permissions', class: 'perms' }),
      ];
    case 'help':
      return [
        h('h2', { text: '도움말' }),
        h('div', { class: 'help' },
          helpItem('배치', '레이아웃에서 컴퓨터를 끌어 실제 모니터가 놓인 대로 맞추세요. 화면끼리 맞닿은 가장자리로 마우스가 넘어갑니다. 초록 선은 넘어갈 수 있는 곳, 빨간 선은 넘어갈 수 없는 곳입니다.'),
          helpItem('확대·축소와 이동', '휠이나 트랙패드 핀치로 확대·축소합니다. 화면을 옮기려면 Space를 누른 채(또는 마우스 가운데 버튼으로) 끄세요. 왼쪽 아래의 퍼센트를 누르면 전체가 보이게 맞춥니다.'),
          helpItem('여러 대 한꺼번에', '빈 곳을 끌어 사각형을 그리면 닿은 컴퓨터가 모두 선택됩니다. Shift를 누르고 컴퓨터를 누르면 하나씩 더하거나 뺄 수 있고, 선택한 것 중 하나를 끌면 모두 같이 움직입니다. Esc나 빈 곳을 누르면 선택이 풀립니다.'),
          helpItem('자세히 보기', '컴퓨터를 누르면 모니터마다 해상도와 배율이 보이고, 오른쪽에 자세한 정보가 뜹니다.'),
          helpItem('되돌아오기', '마우스가 다른 컴퓨터에 있을 때 Ctrl+Shift+Alt+Esc 를 누르면 이 컴퓨터로 돌아옵니다.'),
          helpItem('컴퓨터 추가', '오른쪽 위의 ‘컴퓨터 추가’로 페어링합니다. 두 컴퓨터에 같은 확인 코드가 보이는지 확인하세요.')),
      ];
    default:
      return [
        h('h2', { text: '내 정보' }),
        h('div', { class: 'card account' },
          h('div', { class: 'label', text: '이 컴퓨터' }),
          h('div', { class: 'big', text: snap.this_computer }),
          local ? h('div', { class: 'row' }, osMark(local.os, 16), h('span', { text: OS_NAMES[local.os] || OS_NAMES.other })) : null,
          h('div', { class: 'row' },
            h('span', { class: 'label', text: '지문' }),
            h('code', { class: 'fingerprint', text: snap.fingerprint }),
            h('button', { type: 'button', class: 'small-btn', text: '복사', on: { click: copyFingerprint } }))),
        h('p', {
          class: 'muted',
          text: '페어링할 때 두 컴퓨터에 같은 확인 코드가 보이는지로 서로를 확인합니다. 지문은 직접 비교할 때 쓸 수 있습니다.',
        }),
      ];
  }
}

function helpItem(title, text) {
  return h('div', { class: 'help-item' }, h('strong', { text: title }), h('p', { class: 'muted', text }));
}

function connectionStatus() {
  const client = snap.client;
  switch (client && client.status) {
    case 'connected':
      return [
        h('p', { class: 'ok', text: `‘${client.server}’에 연결되었습니다.` }),
        h('p', { class: 'muted', text: '그 컴퓨터에서 마우스를 이 컴퓨터가 놓인 쪽 화면 가장자리 밖으로 밀면 이쪽으로 넘어옵니다.' }),
      ];
    case 'connecting':
      return [h('div', { class: 'inline' }, h('div', { class: 'spinner' }), '연결하는 중…')];
    case 'waiting':
      return [
        h('div', { class: 'inline tone-warn' }, h('div', { class: 'spinner' }),
          h('strong', { text: '연결이 끊겼습니다. 다시 시도하는 중…' })),
        h('p', { class: 'muted small', text: client.reason }),
      ];
    default:
      return [h('p', { class: 'muted', text: '‘시작’을 누르면 연결합니다.' })];
  }
}

function devicesView(view) {
  view.replaceChildren(h('div', { class: 'stack' },
    h('div', { class: 'section-head' },
      h('h3', { text: '페어링한 컴퓨터' }),
      h('button', { type: 'button', class: 'secondary', text: '컴퓨터 추가', on: { click: () => act('open_pairing') } })),
    snap.devices.length === 0
      ? h('p', { class: 'muted', text: '아직 페어링한 컴퓨터가 없습니다. ‘컴퓨터 추가’로 다른 컴퓨터를 연결하세요.' })
      : snap.devices.map(deviceCard)));
  builtView = 'devices';
}

/** Puts a device on the layout, flush against the right side of what is there. */
function addToLayout(device) {
  const right = Math.max(...snap.screens.map((screen) => screen.x + screen.width));
  const local = snap.screens.find((screen) => screen.local);
  act('place_device', { device: device.name, x: right, y: local ? local.y : 0 });
}

function deviceCard(device) {
  const placed = isPlaced(snap.screens, device.name);
  return h('div', { class: 'card device-row' },
    h('span', { class: `dot ${device.online ? 'good' : ''}` }),
    osMark(device.os),
    h('span', { class: 'name', text: device.name }),
    h('span', { class: 'muted small', text: device.online ? '연결됨' : '오프라인' }),
    h('code', { class: 'muted small grow', text: device.fingerprint_short }),
    h('button', {
      type: 'button',
      class: 'small-btn',
      text: placed ? '배치에서 빼기' : '배치에 추가',
      on: { click: () => (placed ? act('remove_from_layout', { device: device.name }) : addToLayout(device)) },
    }),
    h('button', {
      type: 'button',
      class: 'small-btn bad',
      text: '삭제',
      on: { click: () => { forgetting = device.name; renderOverlay(); } },
    }));
}

async function copyFingerprint() {
  try {
    await navigator.clipboard.writeText(snap.fingerprint);
  } catch {
    // Copying is a convenience; the fingerprint is on screen to be read.
  }
}

function activityView(view) {
  const stuck = view.scrollHeight - view.scrollTop - view.clientHeight < 40 || builtView !== 'activity';
  view.replaceChildren(snap.logs.length === 0
    ? h('p', { class: 'muted', text: '아직 기록이 없습니다.' })
    : h('div', { class: 'log' }, snap.logs.map((line) => h('div', { text: line }))));
  builtView = 'activity';
  if (stuck) view.scrollTop = view.scrollHeight;
}

// ---- dialogs ----

function renderOverlay() {
  const key = JSON.stringify([snap.pairing, forgetting, snap.pairing_port]);
  if (key === overlayKey) return;
  overlayKey = key;

  const overlay = $('overlay');
  const dialog = forgetting ? forgetDialog() : snap.pairing ? pairingDialog() : null;
  overlay.hidden = dialog === null;
  overlay.replaceChildren(...(dialog ? [dialog] : []));
}

function forgetDialog() {
  const name = forgetting;
  const close = () => { forgetting = null; renderOverlay(); };
  return h('div', { class: 'dialog' },
    h('h3', { text: '기기 삭제' }),
    h('p', { text: `‘${name}’의 페어링을 삭제할까요?` }),
    h('p', { class: 'muted small', text: '연결이 바로 끊기고, 다시 페어링하기 전에는 이 컴퓨터에 연결할 수 없습니다.' }),
    h('div', { class: 'actions' },
      h('button', {
        type: 'button',
        class: 'danger',
        text: '삭제',
        on: { click: () => { close(); act('forget_device', { name }); } },
      }),
      h('button', { type: 'button', class: 'secondary', text: '취소', on: { click: close } })));
}

// ---- adding a computer ----
//
// A dialog of its own, with a step for each stage of pairing: choosing how, waiting or
// connecting, comparing the code, and the result.

function addDialog(...children) {
  return h('div', { class: 'dialog add' },
    h('div', { class: 'dialog-head' },
      icon('plus', 18, 'inline'),
      h('h3', { text: '컴퓨터 추가' }),
      iconButton('close', '닫기', () => act('close_pairing'))),
    ...children);
}

function pairingDialog() {
  const pairing = snap.pairing;

  switch (pairing.stage) {
    case 'waiting':
      return addDialog(
        h('div', { class: 'inline' }, h('div', { class: 'spinner' }), '다른 컴퓨터가 연결해 오기를 기다리는 중…'),
        h('p', { text: '다른 컴퓨터에서 ‘컴퓨터 추가’ → ‘다른 컴퓨터에 연결하기’를 고르고 이 주소를 입력하세요:' }),
        h('div', { class: 'address', text: pairingAddress(snap, pairing.addr) }),
        h('div', { class: 'actions' },
          h('button', { type: 'button', class: 'secondary', text: '취소', on: { click: () => act('close_pairing') } })));

    case 'connecting':
      return addDialog(
        h('div', { class: 'inline' }, h('div', { class: 'spinner' }), `${pairing.addr} 에 연결하는 중…`),
        h('div', { class: 'actions' },
          h('button', { type: 'button', class: 'secondary', text: '취소', on: { click: () => act('close_pairing') } })));

    case 'confirming':
      return addDialog(
        h('p', { text: `‘${pairing.peer_name}’ 과(와) 연결되었습니다.` }),
        h('p', { class: 'muted small mono', text: `지문 ${pairing.peer_fingerprint}` }),
        h('p', { text: '두 컴퓨터 화면에 아래와 같은 코드가 보이는지 확인하세요:' }),
        h('div', { class: 'code', text: pairing.code }),
        h('p', { class: 'muted small', text: '코드가 다르면 누군가 중간에서 가로채고 있을 수 있으니 ‘다릅니다’를 누르세요.' }),
        h('div', { class: 'actions' },
          h('button', { type: 'button', class: 'primary', text: '같습니다', on: { click: () => act('answer_pairing', { matches: true }) } }),
          h('button', { type: 'button', class: 'secondary', text: '다릅니다', on: { click: () => act('answer_pairing', { matches: false }) } })));

    case 'done':
      return pairing.ok
        ? addDialog(
          h('p', { class: 'ok', text: `‘${pairing.message}’ 과(와) 페어링되었습니다.` }),
          h('p', { class: 'muted small', text: '이제 ‘기기’ 탭에서 이 컴퓨터를 레이아웃에 추가할 수 있습니다.' }),
          h('div', { class: 'actions' },
            h('button', { type: 'button', class: 'primary', text: '확인', on: { click: () => act('close_pairing') } })))
        : addDialog(
          h('p', { class: 'bad-text', text: '페어링하지 못했습니다.' }),
          h('p', { class: 'muted small', text: pairing.message }),
          h('div', { class: 'actions' },
            h('button', { type: 'button', class: 'primary', text: '다시 시도', on: { click: () => act('open_pairing') } }),
            h('button', { type: 'button', class: 'secondary', text: '닫기', on: { click: () => act('close_pairing') } })));

    default:
      return choosePairingDialog();
  }
}

function choosePairingDialog() {
  const connect = h('button', { type: 'button', class: 'primary', text: '연결' });
  const input = h('input', {
    type: 'text',
    id: 'pair-address',
    value: pairAddress,
    placeholder: '예: 192.168.0.12',
    autocomplete: 'off',
    spellcheck: false,
    on: {
      input: (event) => {
        pairAddress = event.target.value;
        connect.disabled = pairAddress.trim() === '';
      },
      keydown: (event) => {
        if (event.key === 'Enter' && pairAddress.trim() !== '') act('pair_by_connecting', { address: pairAddress });
      },
    },
  });
  connect.disabled = pairAddress.trim() === '';
  connect.addEventListener('click', () => act('pair_by_connecting', { address: pairAddress }));

  return addDialog(
    h('p', { class: 'muted', text: '두 컴퓨터에서 같은 확인 코드가 보이는지로 서로를 확인한 뒤 페어링합니다. 한쪽은 기다리고, 다른 쪽이 연결합니다.' }),
    h('div', { class: 'options' },
      h('div', { class: 'option' },
        h('strong', { text: '이 컴퓨터에서 기다리기' }),
        h('p', { class: 'muted small', text: '다른 컴퓨터에서 이 컴퓨터로 연결해 오게 합니다.' }),
        h('div', { class: 'actions' },
          h('button', { type: 'button', class: 'secondary', text: '기다리기 시작', on: { click: () => act('pair_by_waiting') } }))),
      h('div', { class: 'option' },
        h('strong', { text: '다른 컴퓨터에 연결하기' }),
        h('p', {
          class: 'muted small',
          text: `다른 컴퓨터가 ‘기다리기’ 중일 때, 그 컴퓨터에 표시된 주소를 입력합니다. 포트(${snap.pairing_port})는 생략할 수 있습니다.`,
        }),
        h('div', { class: 'inline' }, input, connect))));
}

// ---- start ----

{
  const add = $('add-computer');
  add.append(icon('plus', 16, 'inline'), '컴퓨터 추가');
  add.addEventListener('click', () => act('open_pairing'));
}

window.addEventListener('keydown', (event) => {
  if (!snap || snap.pairing || forgetting) return;
  // Esc leaves the settings, or lets go of a selection on the layout.
  if (event.key === 'Escape' && tab === 'settings') closeSettings();
  else if (event.key === 'Escape' && tab === 'layout' && !drag && selection.size > 0) deselect();

  // Space, held, makes dragging the canvas move the view. It is left alone while typing.
  if (event.code === 'Space' && tab === 'layout' && !spaceDown && !/^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName || '')) {
    event.preventDefault();
    spaceDown = true;
    document.body.classList.add('space-pan');
  }
});
window.addEventListener('keyup', (event) => {
  if (event.code === 'Space' && spaceDown) {
    spaceDown = false;
    document.body.classList.remove('space-pan');
  }
});
window.addEventListener('blur', () => {
  spaceDown = false;
  document.body.classList.remove('space-pan');
});

let resizeTimer = null;
window.addEventListener('resize', () => {
  // The layout is drawn to fit the canvas, so it is drawn again when the window changes size.
  clearTimeout(resizeTimer);
  resizeTimer = setTimeout(() => {
    if (tab === 'layout' && !drag) render();
  }, 120);
});

refresh();
setInterval(refresh, POLL_MS);
