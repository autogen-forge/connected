// Run with: bun test   (in crates/connected-desktop)
import assert from 'node:assert/strict';
import { test } from 'node:test';

import {
  absDisplays,
  box,
  blockersAt,
  boundsOf,
  contactSegments,
  fitView,
  formatSize,
  isPlaced,
  missingPanes,
  permissionPane,
  permissionSteps,
  monitorFacts,
  overlapRegions,
  pairingAddress,
  shareAddress,
  snapPosition,
  statusOf,
  statusBarOf,
  tabsFor,
  toCanvas,
  toDesktop,
  unreachable,
  zoomAt,
  panBy,
} from './logic.js';

const sharing = (extra = {}) => ({
  mode: 'share',
  this_computer: 'desk',
  lan_address: '192.168.1.5',
  server: { listening_on: '0.0.0.0:24900', active_screen: 'desk', clients: [] },
  client: null,
  ...extra,
});

test('a computer that is not sharing says it is stopped', () => {
  assert.deepEqual(statusOf(sharing({ server: null })), { tone: 'muted', text: '중지됨' });
});

test('sharing with nobody connected is a warning', () => {
  assert.deepEqual(statusOf(sharing()), { tone: 'warn', text: '공유 중 · 연결된 컴퓨터 없음' });
});

test('sharing with computers connected counts them', () => {
  const clients = [{ name: 'a' }, { name: 'b' }];
  const snap = sharing({ server: { listening_on: ':1', active_screen: 'desk', clients } });
  assert.deepEqual(statusOf(snap), { tone: 'good', text: '공유 중 · 컴퓨터 2대 연결됨' });
});

test('while the cursor is on another computer, the status names it', () => {
  const snap = sharing({ server: { listening_on: ':1', active_screen: 'laptop', clients: [{}] } });
  assert.deepEqual(statusOf(snap), { tone: 'accent', text: '‘laptop’ 조작 중' });
});

test('a receiving computer reports each state of its link', () => {
  const receiving = (client) => statusOf({ mode: 'receive', this_computer: 'x', client });
  assert.equal(receiving({ status: 'connected', server: 'desk' }).text, '‘desk’에 연결됨');
  assert.equal(receiving({ status: 'connecting' }).tone, 'warn');
  assert.equal(receiving({ status: 'waiting', reason: 'refused' }).text, '연결 끊김 · 다시 시도하는 중');
  assert.equal(receiving(null).text, '중지됨');
  assert.equal(receiving({ status: 'failed', reason: 'x' }).tone, 'muted');
});

const screen = (name, x, y, width = 100, height = 100) => ({ name, x, y, width, height });

test('the bounds hold every screen', () => {
  assert.deepEqual(boundsOf([screen('a', 0, 0), screen('b', 150, -20, 50, 50)]), {
    x: 0,
    y: -20,
    width: 200,
    height: 120,
  });
});

test('a view fits all the screens on the canvas with room to spare', () => {
  const screens = [screen('a', 0, 0, 1920, 1080), screen('b', 1920, 0, 2560, 1440)];
  const view = fitView(screens, 800, 500);
  for (const s of screens) {
    const topLeft = toCanvas(view, s.x, s.y);
    const bottomRight = toCanvas(view, s.x + s.width, s.y + s.height);
    assert.ok(topLeft.x > 0 && topLeft.y > 0, 'inside at the top left');
    assert.ok(bottomRight.x < 800 && bottomRight.y < 500, 'inside at the bottom right');
  }
  assert.ok(view.scale > 0);
});

test('one screen alone is not blown up to fill the canvas', () => {
  const view = fitView([screen('a', 0, 0, 800, 600)], 2000, 1500);
  assert.equal(view.scale, 0.2);
});

test('canvas and desktop coordinates convert both ways', () => {
  const view = fitView([screen('a', 0, 0, 1920, 1080)], 800, 500);
  const there = toCanvas(view, 960, 540);
  const back = toDesktop(view, there.x, there.y);
  assert.ok(Math.abs(back.x - 960) < 1e-9 && Math.abs(back.y - 540) < 1e-9);
  assert.ok(Math.abs(there.x - 400) < 1e-9 && Math.abs(there.y - 250) < 1e-9, 'centered');
});

/** A computer with several displays, given relative to its corner. */
const group = (name, x, y, displays) => {
  const right = Math.max(...displays.map((d) => d.x + d.width));
  const bottom = Math.max(...displays.map((d) => d.y + d.height));
  return { name, x, y, width: right, height: bottom, displays };
};
const d = (x, y, width, height) => ({ x, y, width, height });

test('displays are placed at the computer\'s corner', () => {
  const pc = group('pc', 1000, 200, [d(0, 0, 100, 100), d(100, 50, 80, 80)]);
  assert.deepEqual(absDisplays(pc), [d(1000, 200, 100, 100), d(1100, 250, 80, 80)]);
  assert.deepEqual(absDisplays(pc, 0, 0), [d(0, 0, 100, 100), d(100, 50, 80, 80)]);
});

test('fitting keeps the screens out from under the floating panels', () => {
  const screens = [screen('a', 0, 0, 1920, 1080)];
  const view = fitView(screens, 1000, 600, [], { right: 300, top: 100 });
  const corner = toCanvas(view, 1920, 0);
  const bottom = toCanvas(view, 0, 1080);
  assert.ok(corner.x < 700, 'left of the panel on the right');
  assert.ok(toCanvas(view, 0, 0).y > 100, 'below the panel on top');
  assert.ok(bottom.y < 600);
});

test('zooming keeps the point under the pointer where it is', () => {
  const view = fitView([screen('a', 0, 0, 1920, 1080)], 800, 500);
  const before = toDesktop(view, 300, 200);
  const zoomed = zoomAt(view, 2, 300, 200, 0.001, 1);
  assert.ok(Math.abs(zoomed.scale - view.scale * 2) < 1e-12);
  const after = toDesktop(zoomed, 300, 200);
  assert.ok(Math.abs(after.x - before.x) < 1e-6 && Math.abs(after.y - before.y) < 1e-6);
});

test('zooming stops at the limits', () => {
  const view = { scale: 0.1, originX: 0, originY: 0 };
  assert.equal(zoomAt(view, 100, 0, 0, 0.01, 0.5).scale, 0.5);
  assert.equal(zoomAt(view, 0.0001, 0, 0, 0.01, 0.5).scale, 0.01);
});

test('dragging the canvas moves the screens the same way', () => {
  const view = { scale: 0.1, originX: 100, originY: 50 };
  const panned = panBy(view, 30, -20);
  const before = toCanvas(view, 500, 500);
  const after = toCanvas(panned, 500, 500);
  assert.ok(Math.abs(after.x - before.x - 30) < 1e-9);
  assert.ok(Math.abs(after.y - before.y + 20) < 1e-9);
  assert.equal(panned.scale, 0.1);
});

test('touching displays do not overlap, crossing ones do, and the overlap is the shared area', () => {
  const a = [d(0, 0, 100, 100)];
  assert.deepEqual(overlapRegions([d(100, 0, 100, 100)], a), []);
  assert.deepEqual(overlapRegions([d(0, 100, 100, 100)], a), []);
  assert.deepEqual(overlapRegions([d(99, 50, 100, 100)], a), [d(99, 50, 1, 50)]);
  assert.deepEqual(overlapRegions([d(-50, -50, 100, 100)], a), [d(0, 0, 50, 50)]);
  assert.deepEqual(overlapRegions([d(500, 500, 100, 100)], a), []);
});

test('a computer takes up the rectangle around all its displays', () => {
  const pc = group('pc', 10, 20, [d(0, 0, 100, 50), d(0, 50, 50, 50)]);
  assert.deepEqual(box(pc), { x: 10, y: 20, width: 100, height: 100 });
  assert.deepEqual(box(pc, 0, 0), { x: 0, y: 0, width: 100, height: 100 });
});

test('every pair of displays is compared, so the overlaps of a group come out separately', () => {
  const mine = [d(0, 0, 100, 100), d(100, 0, 100, 100)];
  const regions = overlapRegions(mine, [d(50, 20, 100, 30)]);
  assert.deepEqual(regions, [d(50, 20, 50, 30), d(100, 20, 50, 30)]);
});

test('the computers a screen would sit on are named', () => {
  const here = screen('a', 0, 0);
  const near = { ...screen('near', 300, 0), displays: [d(0, 0, 100, 100)] };
  const wide = group('wide', 90, 0, [d(0, 0, 50, 50), d(300, 0, 50, 50)]);
  const withDisplays = (s) => ({ ...s, displays: [d(0, 0, s.width, s.height)] });
  assert.deepEqual(blockersAt(withDisplays(here), 0, 0, [near, wide]).map((s) => s.name), ['wide']);
  assert.deepEqual(blockersAt(withDisplays(here), 300, 0, [near, wide]).map((s) => s.name), ['near', 'wide']);
  assert.deepEqual(blockersAt(withDisplays(here), 1000, 1000, [near, wide]), []);
});

const single = (name, x, y, width = 100, height = 100) => group(name, x, y, [d(0, 0, width, height)]);

test('a screen dropped near another settles flush against it', () => {
  const other = single('other', 0, 0, 1920, 1080);
  const moving = single('x', 0, 0, 1000, 800);
  // Close to the right edge, a little too far and a little low.
  assert.deepEqual(snapPosition(moving, 1950, 30, [other], 60), { x: 1920, y: 0 });
  // Overlapping it a little on the right.
  assert.deepEqual(snapPosition(moving, 1900, 700, [other], 60), { x: 1920, y: 700 });
  // To the left of it.
  assert.deepEqual(snapPosition(moving, -1030, 20, [other], 60), { x: -1000, y: 0 });
});

test('a screen below another lines up with its left edge or flush under it', () => {
  const other = single('other', 0, 0, 1920, 1080);
  const moving = single('x', 0, 0, 800, 600);
  assert.deepEqual(snapPosition(moving, 25, 1100, [other], 60), { x: 0, y: 1080 });
  assert.deepEqual(snapPosition(moving, 1150, 1050, [other], 60), { x: 1120, y: 1080 });
});

test('a screen far from everything stays where it was dropped', () => {
  const other = single('other', 0, 0, 1920, 1080);
  assert.deepEqual(snapPosition(single('x', 0, 0, 500, 500), 3000, 2000, [other], 60), { x: 3000, y: 2000 });
  assert.deepEqual(snapPosition(single('x', 0, 0, 5, 5), 10, 20, [], 60), { x: 10, y: 20 });
});

test('snapping does not move a screen that is already further than the threshold', () => {
  const other = single('other', 0, 0, 1000, 1000);
  assert.deepEqual(snapPosition(single('x', 0, 0, 500, 500), 1100, 0, [other], 60), { x: 1100, y: 0 });
});

test('a group snaps by its whole rectangle, however its displays sit inside it', () => {
  const other = single('other', 0, 0, 1000, 1000);
  const pair = group('pair', 0, 0, [d(0, 0, 400, 400), d(400, 300, 400, 400)]);
  // The first display is 30 from the other's right edge.
  assert.deepEqual(snapPosition(pair, 1030, 0, [other], 60), { x: 1000, y: 0 });
  // The second display is the one 30 short of the other's left edge, far from the first.
  assert.deepEqual(snapPosition(pair, -830, 0, [other], 60), { x: -800, y: 0 });
  // The first display is 30 below the other's bottom edge.
  assert.deepEqual(snapPosition(pair, 200, 1030, [other], 60), { x: 200, y: 1000 });
});

const seg = (a, b, ok, x1, y1, x2, y2, capStart = true, capEnd = true) => ({
  a, b, ok, x1, y1, x2, y2, capStart, capEnd,
});

test('side by side screens touch as far as their rectangles overlap', () => {
  const edges = contactSegments([single('a', 0, 0, 100, 100), single('b', 100, 40, 100, 100)]);
  assert.deepEqual(edges, [seg('a', 'b', true, 100, 40, 100, 100)]);
});

test('stacked screens touch along a horizontal line, in either order', () => {
  const below = contactSegments([single('a', 0, 0, 100, 100), single('b', 50, 100, 100, 100)]);
  assert.deepEqual(below, [seg('a', 'b', true, 50, 100, 100, 100)]);
  const above = contactSegments([single('a', 0, 100, 100, 100), single('b', 0, 0, 100, 100)]);
  assert.deepEqual(above, [seg('a', 'b', true, 0, 100, 100, 100)]);
});

test('screens that only meet at a corner, or not at all, touch nowhere', () => {
  assert.deepEqual(contactSegments([single('a', 0, 0), single('b', 100, 100)]), []);
  assert.deepEqual(contactSegments([single('a', 0, 0), single('b', 150, 0)]), []);
});

test('the displays of one computer do not touch each other', () => {
  const pc = group('pc', 0, 0, [d(0, 0, 100, 100), d(100, 0, 100, 100)]);
  assert.deepEqual(contactSegments([pc]), []);
});

test('the cursor passes where a display faces a display, and not where it faces an empty corner', () => {
  // The second display is lower, so above it the computer's rectangle has an empty corner.
  const pc = group('pc', 0, 0, [d(0, 0, 100, 100), d(100, 50, 100, 100)]);
  const tv = single('tv', 200, 0, 50, 100);
  assert.deepEqual(contactSegments([pc, tv]), [
    seg('pc', 'tv', false, 200, 0, 200, 50, true, false),
    seg('pc', 'tv', true, 200, 50, 200, 100, false, true),
  ]);
});

test('a neighbor that sits entirely beside a display is passable all along', () => {
  const pc = group('pc', 0, 0, [d(0, 0, 100, 100), d(100, 50, 100, 100)]);
  assert.deepEqual(contactSegments([pc, single('tv', 200, 80, 50, 50)]), [
    seg('pc', 'tv', true, 200, 80, 200, 130),
  ]);
});

test('both sides count: an empty corner on the other computer blocks too', () => {
  const left = single('left', 0, 0, 100, 100);
  const pc = group('pc', 100, 0, [d(0, 0, 50, 50), d(50, 50, 50, 50)]);
  assert.deepEqual(contactSegments([left, pc]), [
    seg('left', 'pc', true, 100, 0, 100, 50, true, false),
    seg('left', 'pc', false, 100, 50, 100, 100, false, true),
  ]);
});

test('a computer touched only at an empty corner cannot be reached', () => {
  const pc = group('pc', 0, 0, [d(0, 0, 100, 50), d(0, 50, 50, 50)]);
  const tv = single('tv', 100, 50, 50, 50);
  assert.deepEqual(contactSegments([pc, tv]), [seg('pc', 'tv', false, 100, 50, 100, 100)]);
  assert.deepEqual(unreachable([pc, tv], 'pc'), ['tv']);
});

test('a screen is unreachable unless a chain of shared edges leads to it', () => {
  const chain = [single('desk', 0, 0), single('a', 100, 0), single('b', 200, 0), single('far', 500, 0)];
  assert.deepEqual(unreachable(chain, 'desk'), ['far']);
  assert.deepEqual(unreachable(chain, 'far'), ['desk', 'a', 'b']);
  assert.deepEqual(unreachable([single('desk', 0, 0)], 'desk'), []);
});

test('a spot knows whether a device is on the layout', () => {
  const screens = [single('desk', 0, 0), single('laptop', 100, 0)];
  assert.equal(isPlaced(screens, 'laptop'), true);
  assert.equal(isPlaced(screens, 'tablet'), false);
  assert.equal(isPlaced(screens, 'constructor'), false);
});

test('a monitor reports its resolution in pixels, its magnification, and whether it is the main one', () => {
  // 1512 x 982 points at 200% is a 3024 x 1964 display.
  assert.deepEqual(monitorFacts(d(0, 0, 1512, 982), 200, 0), { resolution: '3024 × 1964', scale: '200%', main: true });
  assert.deepEqual(monitorFacts(d(0, 0, 1280, 720), 150, 1), { resolution: '1920 × 1080', scale: '150%', main: false });
  // Not told, it is taken as not magnified.
  assert.deepEqual(monitorFacts(d(0, 0, 1920, 1080), undefined, 1), { resolution: '1920 × 1080', scale: '100%', main: false });
});

test('screen sizes read as width by height', () => {
  assert.equal(formatSize([1470, 956]), '1470 × 956');
  assert.equal(formatSize(null), null);
});

test('the address to type is the local network address with the server port', () => {
  assert.equal(shareAddress(sharing()), '192.168.1.5:24900');
  assert.equal(shareAddress(sharing({ lan_address: null })), '0.0.0.0:24900');
  assert.equal(shareAddress(sharing({ server: null })), null);
  assert.equal(pairingAddress({ lan_address: '10.0.0.2' }, '0.0.0.0:24901'), '10.0.0.2:24901');
  assert.equal(pairingAddress({ lan_address: null }, '0.0.0.0:24901'), '0.0.0.0:24901');
});

test('the navigation bar has the same four tabs in either mode', () => {
  assert.deepEqual(tabsFor().map(([id]) => id), ['layout', 'devices', 'activity', 'settings']);
  assert.deepEqual(tabsFor().map(([, label]) => label), ['레이아웃', '기기', '활동', '설정']);
});

const devices = (count) => Array.from({ length: count }, (_, i) => ({ name: `d${i}` }));

test('the status bar says whether this computer is sharing and how many are online', () => {
  const snap = sharing({
    devices: devices(4),
    server: { listening_on: '0.0.0.0:24900', active_screen: 'desk', clients: [{ name: 'a' }, { name: 'b' }] },
  });
  assert.deepEqual(statusBarOf(snap), {
    items: [
      { tone: 'good', text: '공유 중' },
      { tone: 'good', text: '온라인 2 / 4대' },
    ],
    address: '192.168.1.5:24900',
  });
});

test('the status bar says whose input it is when the cursor is on another computer', () => {
  const snap = sharing({
    devices: devices(1),
    server: { listening_on: '0.0.0.0:24900', active_screen: 'laptop', clients: [{ name: 'laptop' }] },
  });
  assert.deepEqual(statusBarOf(snap).items.at(-1), { tone: 'accent', text: '‘laptop’ 조작 중' });
});

test('a sharing computer with nobody online is a warning, and a stopped one just counts', () => {
  const idle = statusBarOf(sharing({
    devices: devices(2),
    server: { listening_on: ':1', active_screen: 'desk', clients: [] },
  }));
  assert.deepEqual(idle.items, [
    { tone: 'warn', text: '연결 대기 중' },
    { tone: 'warn', text: '온라인 0 / 2대' },
  ]);

  const stopped = statusBarOf(sharing({ devices: devices(3), server: null }));
  assert.deepEqual(stopped, {
    items: [{ tone: 'muted', text: '공유 중지됨' }, { tone: 'muted', text: '페어링 3대' }],
    address: null,
  });
  assert.equal(statusBarOf(sharing({ devices: [], server: null })).items[1].text, '페어링한 컴퓨터 없음');
});

test('a receiving computer shows how it is connected, and has no address', () => {
  const bar = statusBarOf({
    mode: 'receive',
    this_computer: 'laptop',
    devices: [],
    server: null,
    client: { status: 'connected', server: 'desk' },
  });
  assert.deepEqual(bar, { items: [{ tone: 'good', text: '‘desk’에 연결됨' }], address: null });
});

test('each permission is given on its own page of the system settings', () => {
  assert.deepEqual(permissionPane('Accessibility'), { pane: 'accessibility', label: '손쉬운 사용' });
  assert.deepEqual(permissionPane('Post Events'), { pane: 'accessibility', label: '손쉬운 사용' });
  assert.deepEqual(permissionPane('Input Monitoring'), { pane: 'input_monitoring', label: '입력 모니터링' });
});

test('only the pages of missing permissions are offered, and each once', () => {
  const permissions = [
    { name: 'Accessibility', granted: false },
    { name: 'Input Monitoring', granted: false },
    { name: 'Post Events', granted: false },
  ];
  assert.deepEqual(missingPanes(permissions).map((page) => page.pane), ['accessibility', 'input_monitoring']);
  permissions[0].granted = true;
  permissions[2].granted = true;
  assert.deepEqual(missingPanes(permissions).map((page) => page.pane), ['input_monitoring']);
  permissions[1].granted = true;
  assert.deepEqual(missingPanes(permissions), []);
});

test('permissions are given in steps, one for each page of the system settings', () => {
  const permissions = [
    { name: 'Accessibility', granted: false },
    { name: 'Input Monitoring', granted: false },
    { name: 'Post Events', granted: false },
  ];
  const first = permissionSteps(permissions);
  assert.deepEqual(first.steps.map((step) => [step.pane, step.names, step.done]), [
    ['accessibility', ['Accessibility', 'Post Events'], false],
    ['input_monitoring', ['Input Monitoring'], false],
  ]);
  assert.equal(first.current, 0);
});

test('a step is done only when everything on its page is given', () => {
  const second = permissionSteps([
    { name: 'Accessibility', granted: true },
    { name: 'Input Monitoring', granted: false },
    { name: 'Post Events', granted: false },
  ]);
  assert.equal(second.steps[0].done, false, 'Post Events is still missing');
  assert.equal(second.current, 0);

  const third = permissionSteps([
    { name: 'Accessibility', granted: true },
    { name: 'Input Monitoring', granted: false },
    { name: 'Post Events', granted: true },
  ]);
  assert.equal(third.steps[0].done, true);
  assert.equal(third.current, 1, 'on to the next page');
});

test('with every permission given there is no current step', () => {
  const done = permissionSteps([{ name: 'Accessibility', granted: true }, { name: 'Input Monitoring', granted: true }]);
  assert.equal(done.current, -1);
  assert.deepEqual(permissionSteps([]), { steps: [], current: -1 });
});
