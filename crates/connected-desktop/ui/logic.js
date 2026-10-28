// Everything the page works out from a snapshot, apart from drawing it. Kept free of the
// page itself so that it can be tested with `node --test`.

/** What to say about the current state, and in what tone (muted, good, warn or accent). */
export function statusOf(snap) {
  if (snap.mode === 'share') {
    const server = snap.server;
    if (!server) return { tone: 'muted', text: '중지됨' };
    if (server.active_screen !== snap.this_computer) {
      return { tone: 'accent', text: `‘${server.active_screen}’ 조작 중` };
    }
    const count = server.clients.length;
    if (count === 0) return { tone: 'warn', text: '공유 중 · 연결된 컴퓨터 없음' };
    return { tone: 'good', text: `공유 중 · 컴퓨터 ${count}대 연결됨` };
  }

  const client = snap.client;
  switch (client && client.status) {
    case 'connected':
      return { tone: 'good', text: `‘${client.server}’에 연결됨` };
    case 'connecting':
      return { tone: 'warn', text: '연결하는 중…' };
    case 'waiting':
      return { tone: 'warn', text: '연결 끊김 · 다시 시도하는 중' };
    default:
      return { tone: 'muted', text: '중지됨' };
  }
}

// ---- the layout canvas ----
//
// Screens are rectangles `{ name, x, y, width, height }` in pixels on one shared desktop. The
// canvas shows that desktop scaled down; these functions convert between the two and decide
// where a dragged screen settles.

/** The smallest rectangle holding all of `rects`. */
export function boundsOf(rects) {
  const left = Math.min(...rects.map((r) => r.x));
  const top = Math.min(...rects.map((r) => r.y));
  const right = Math.max(...rects.map((r) => r.x + r.width));
  const bottom = Math.max(...rects.map((r) => r.y + r.height));
  return { x: left, y: top, width: right - left, height: bottom - top };
}

/** The most a desktop pixel is magnified on the canvas: one screen alone should not fill it. */
const MAX_SCALE = 0.2;

/**
 * How to draw `rects` on a canvas of the given size: `screen = (desktop - origin) * scale`.
 * There is room around the screens, as wide as the biggest of them (`spare` lists the sizes
 * of screens not on the desktop yet), so that another can be dropped on any side.
 */
export function fitView(rects, canvasWidth, canvasHeight, spare = [], insets = {}) {
  const { left = 0, right = 0, top = 0, bottom = 0 } = insets;
  // The part of the canvas that is not under a floating panel.
  const width = Math.max(40, canvasWidth - left - right);
  const height = Math.max(40, canvasHeight - top - bottom);
  const bounds = boundsOf(rects);
  const room = Math.max(...rects.concat(spare).map((r) => Math.max(r.width, r.height))) * 0.4;
  const scale = Math.min(
    MAX_SCALE,
    width / (bounds.width + room * 2),
    height / (bounds.height + room * 2),
  );
  // The middle of the screens goes to the middle of that part.
  return {
    scale,
    originX: bounds.x + bounds.width / 2 - (left + width / 2) / scale,
    originY: bounds.y + bounds.height / 2 - (top + height / 2) / scale,
  };
}

/**
 * The view after zooming by `factor` about the canvas position (`px`, `py`): the desktop
 * point under that position stays under it. The scale is kept between `min` and `max`.
 */
export function zoomAt(view, factor, px, py, min, max) {
  const scale = Math.min(max, Math.max(min, view.scale * factor));
  const at = toDesktop(view, px, py);
  return { scale, originX: at.x - px / scale, originY: at.y - py / scale };
}

/** The view after dragging the canvas by (`dx`, `dy`) pixels. */
export function panBy(view, dx, dy) {
  return { ...view, originX: view.originX - dx / view.scale, originY: view.originY - dy / view.scale };
}

export function toCanvas(view, x, y) {
  return { x: (x - view.originX) * view.scale, y: (y - view.originY) * view.scale };
}

export function toDesktop(view, px, py) {
  return { x: px / view.scale + view.originX, y: py / view.scale + view.originY };
}

/**
 * A computer's displays on the shared desktop, as rectangles. A computer is `{ name, x, y,
 * width, height, displays }` where `displays` are measured from (`x`, `y`), the corner of all
 * of them; `x` and `y` can be given to ask where they would be if the computer sat elsewhere.
 */
export function absDisplays(screen, x = screen.x, y = screen.y) {
  return screen.displays.map((d) => ({ x: x + d.x, y: y + d.y, width: d.width, height: d.height }));
}

/** The areas where rectangles of `mine` and of `others` cover each other. Touching is fine. */
export function overlapRegions(mine, others) {
  const regions = [];
  for (const a of mine) {
    for (const b of others) {
      const x = Math.max(a.x, b.x);
      const y = Math.max(a.y, b.y);
      const width = Math.min(a.x + a.width, b.x + b.width) - x;
      const height = Math.min(a.y + a.height, b.y + b.height) - y;
      if (width > 0 && height > 0) regions.push({ x, y, width, height });
    }
  }
  return regions;
}

/** The rectangle a computer takes up: the one holding all its displays, empty corners too. */
export function box(screen, x = screen.x, y = screen.y) {
  return { x, y, width: screen.width, height: screen.height };
}

/** The computers among `others` that `screen`, put at (`x`, `y`), would sit on top of. */
export function blockersAt(screen, x, y, others) {
  const mine = [box(screen, x, y)];
  return others.filter((other) => overlapRegions(mine, [box(other)]).length > 0);
}

/**
 * Where a computer settles when dropped with its corner at (`x`, `y`) near other computers:
 * its edges jump onto the edges of computers that are within `threshold` pixels, so they end
 * up flush with each other or lined up. Returns the new corner.
 */
export function snapPosition(screen, x, y, others, threshold) {
  const along = (lo1, hi1, lo2, hi2) => lo1 < hi2 + threshold && lo2 < hi1 + threshold;
  const nearest = (candidates) =>
    candidates.reduce(
      (best, d) => (Math.abs(d) <= threshold && (best === null || Math.abs(d) < Math.abs(best)) ? d : best),
      null,
    );
  const m = box(screen, x, y);

  const xs = [];
  for (const o of others.map((other) => box(other))) {
    if (!along(m.y, m.y + m.height, o.y, o.y + o.height)) continue;
    xs.push(o.x + o.width - m.x); // left edge against its right edge
    xs.push(o.x - (m.x + m.width)); // right edge against its left edge
    xs.push(o.x - m.x); // left edges lined up
    xs.push(o.x + o.width - (m.x + m.width)); // right edges lined up
  }
  const dx = nearest(xs) ?? 0;

  const ys = [];
  for (const o of others.map((other) => box(other))) {
    if (!along(m.x + dx, m.x + dx + m.width, o.x, o.x + o.width)) continue;
    ys.push(o.y + o.height - m.y);
    ys.push(o.y - (m.y + m.height));
    ys.push(o.y - m.y);
    ys.push(o.y + o.height - (m.y + m.height));
  }
  const dy = nearest(ys) ?? 0;
  return { x: Math.round(x + dx), y: Math.round(y + dy) };
}

// ---- where the cursor can pass ----

/** Sorts intervals `[from, to]` and joins those that touch or overlap. */
function merged(intervals) {
  const sorted = intervals.filter(([from, to]) => to > from).sort((a, b) => a[0] - b[0]);
  const result = [];
  for (const [from, to] of sorted) {
    const last = result[result.length - 1];
    if (last && from <= last[1]) last[1] = Math.max(last[1], to);
    else result.push([from, to]);
  }
  return result;
}

function intersection(a, b) {
  const result = [];
  for (const [from1, to1] of a) {
    for (const [from2, to2] of b) {
      const from = Math.max(from1, from2);
      const to = Math.min(to1, to2);
      if (to > from) result.push([from, to]);
    }
  }
  return merged(result);
}

/** What is left of `[from, to]` once the intervals in `taken` (sorted, apart) are removed. */
function remainder([from, to], taken) {
  const result = [];
  let at = from;
  for (const [start, end] of taken) {
    if (start > at) result.push([at, start]);
    at = Math.max(at, end);
  }
  if (at < to) result.push([at, to]);
  return result;
}

/**
 * Where computers touch, split by whether the cursor can really pass there:
 * `{ a, b, ok, x1, y1, x2, y2, capStart, capEnd }`, a vertical line where they sit side by side, a horizontal one
 * where they are stacked. Computers are rectangles, so they touch along the whole stretch
 * where their rectangles meet; but the cursor can only pass where a display of each faces a
 * display of the other (`ok`). Elsewhere, a corner of one faces an empty corner of the other.
 */
export function contactSegments(screens) {
  const segments = [];
  for (let i = 0; i < screens.length; i += 1) {
    for (let j = i + 1; j < screens.length; j += 1) {
      const p = screens[i];
      const q = screens[j];
      const sides = [
        ['v', p, q, p.x + p.width === q.x],
        ['v', q, p, q.x + q.width === p.x],
        ['h', p, q, p.y + p.height === q.y],
        ['h', q, p, q.y + q.height === p.y],
      ];
      for (const [direction, first, second, touching] of sides) {
        if (!touching) continue;
        const vertical = direction === 'v';
        // `first` is left of (or above) `second`; the line is where they meet.
        const line = vertical ? first.x + first.width : first.y + first.height;
        const lo = vertical ? Math.max(first.y, second.y) : Math.max(first.x, second.x);
        const hi = vertical
          ? Math.min(first.y + first.height, second.y + second.height)
          : Math.min(first.x + first.width, second.x + second.width);
        if (hi <= lo) continue;

        const faces = (screen, atEnd) =>
          merged(
            absDisplays(screen)
              .filter((d) => (vertical ? (atEnd ? d.x + d.width : d.x) : atEnd ? d.y + d.height : d.y) === line)
              .map((d) => (vertical ? [d.y, d.y + d.height] : [d.x, d.x + d.width])),
          );
        const open = intersection(faces(first, true), faces(second, false));
        const parts = [
          ...open.map(([from, to]) => ({ ok: true, from, to })),
          ...remainder([lo, hi], open).map(([from, to]) => ({ ok: false, from, to })),
        ].sort((a, b) => a.from - b.from);
        // Only the two ends of the whole contact are rounded; where one stretch meets the next
        // (a green one and a red one), the line goes straight on.
        parts.forEach(({ ok, from, to }, index) => {
          const caps = { capStart: index === 0, capEnd: index === parts.length - 1 };
          segments.push(
            vertical
              ? { a: p.name, b: q.name, ok, x1: line, y1: from, x2: line, y2: to, ...caps }
              : { a: p.name, b: q.name, ok, x1: from, y1: line, x2: to, y2: line, ...caps },
          );
        });
      }
    }
  }
  return segments;
}

/** Names of screens the cursor cannot reach from this computer's, going from screen to screen. */
export function unreachable(screens, localName) {
  const edges = contactSegments(screens).filter((segment) => segment.ok);
  const reached = new Set([localName]);
  for (let grew = true; grew; ) {
    grew = false;
    for (const edge of edges) {
      const a = reached.has(edge.a);
      const b = reached.has(edge.b);
      if (a !== b) {
        reached.add(edge.a).add(edge.b);
        grew = true;
      }
    }
  }
  return screens.map((screen) => screen.name).filter((name) => !reached.has(name));
}

export function isPlaced(screens, name) {
  return screens.some((screen) => screen.name === name);
}

/**
 * What to say about one monitor of a computer: its resolution in pixels (its size on the
 * desktop times its magnification), the magnification, and whether it is the main monitor,
 * which is the first. `scale` is in percent and missing means none.
 */
export function monitorFacts(display, scale, index) {
  const percent = scale > 0 ? scale : 100;
  const pixels = [Math.round((display.width * percent) / 100), Math.round((display.height * percent) / 100)];
  return { resolution: formatSize(pixels), scale: `${percent}%`, main: index === 0 };
}

export function formatSize(screen) {
  return screen ? `${screen[0]} × ${screen[1]}` : null;
}

/** The address other computers should type to reach this one, once it is sharing. */
export function shareAddress(snap) {
  if (!snap.server) return null;
  const port = snap.server.listening_on.split(':').pop();
  return snap.lan_address ? `${snap.lan_address}:${port}` : snap.server.listening_on;
}

/** The address a waiting computer shows for pairing. */
export function pairingAddress(snap, listeningOn) {
  const port = String(listeningOn).split(':').pop();
  return snap.lan_address ? `${snap.lan_address}:${port}` : listeningOn;
}

/**
 * Which page of the system settings a permission is given on, by the name the app reports it
 * under: `{ pane, label }`. Posting events is allowed together with the other accessibility
 * features; reading input has a page of its own.
 */
export function permissionPane(name) {
  if (name === 'Input Monitoring') return { pane: 'input_monitoring', label: '입력 모니터링' };
  return { pane: 'accessibility', label: '손쉬운 사용' };
}

/**
 * The steps of giving permissions, one for each page of the system settings, in the order the
 * permissions are listed: `{ pane, label, names, done }`. `current` is the first step not yet
 * done, or -1 when every permission is given.
 */
export function permissionSteps(permissions) {
  const steps = [];
  for (const permission of permissions) {
    const page = permissionPane(permission.name);
    let step = steps.find((known) => known.pane === page.pane);
    if (!step) {
      step = { ...page, names: [], done: true };
      steps.push(step);
    }
    step.names.push(permission.name);
    if (!permission.granted) step.done = false;
  }
  return { steps, current: steps.findIndex((step) => !step.done) };
}

/** The settings pages to visit for the permissions that are missing, each once. */
export function missingPanes(permissions) {
  const panes = [];
  for (const permission of permissions) {
    if (permission.granted) continue;
    const page = permissionPane(permission.name);
    if (!panes.some((known) => known.pane === page.pane)) panes.push(page);
  }
  return panes;
}

/** The tabs of the navigation bar, as `[id, label]` pairs. */
export function tabsFor() {
  return [
    ['layout', '레이아웃'],
    ['devices', '기기'],
    ['activity', '활동'],
    ['settings', '설정'],
  ];
}

/**
 * What the status bar at the bottom says: a list of `{ tone, text }` (tone being muted, good,
 * warn or accent), and the address other computers connect to, when this one is sharing.
 */
export function statusBarOf(snap) {
  const items = [];
  const total = snap.devices.length;

  if (snap.mode === 'share') {
    const server = snap.server;
    // Listening is not yet sharing: until another computer has connected, nothing is shared.
    if (!server) items.push({ tone: 'muted', text: '공유 중지됨' });
    else if (server.clients.length === 0) items.push({ tone: 'warn', text: '연결 대기 중' });
    else items.push({ tone: 'good', text: '공유 중' });
    if (server) {
      items.push({
        tone: server.clients.length > 0 ? 'good' : 'warn',
        text: total === 0 ? '페어링한 컴퓨터 없음' : `온라인 ${server.clients.length} / ${total}대`,
      });
      if (server.active_screen !== snap.this_computer) {
        items.push({ tone: 'accent', text: `‘${server.active_screen}’ 조작 중` });
      }
    } else {
      items.push({ tone: 'muted', text: total === 0 ? '페어링한 컴퓨터 없음' : `페어링 ${total}대` });
    }
    return { items, address: shareAddress(snap) };
  }

  const status = statusOf(snap);
  items.push({ tone: status.tone, text: status.text });
  return { items, address: null };
}
