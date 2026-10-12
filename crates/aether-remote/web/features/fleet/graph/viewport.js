import { button, el, invariant } from '../../../ui/dom.js';

/** @typedef {{x:number,y:number,scale:number}} Camera */
/** @param {SVGSVGElement} canvas @param {SVGGElement} scene */
export function createViewport(canvas, scene) {
  invariant(canvas.contains(scene), 'The graph scene must belong to its canvas');
  invariant(typeof ResizeObserver === 'function', 'Graph viewport requires resize observation');
  let bounds = { width: 300, height: 300, readableScale: 1 };
  let mode = 'readable';
  const camera = { x: 0, y: 0, scale: 1 };
  const status = el('span', 'fleet-graph-viewport-status');
  const render = () => {
    scene.setAttribute('transform', `translate(${camera.x} ${camera.y}) scale(${camera.scale})`);
    const clipped = bounds.height * camera.scale > canvas.clientHeight || bounds.width * camera.scale > canvas.clientWidth;
    status.textContent = `${Math.round(camera.scale * 100)}%${clipped ? ' · Drag to explore' : ''}`;
  };
  const fit = () => {
    const { width, height } = canvas.getBoundingClientRect();
    if (!width || !height) return;
    const overview = Math.max(.02, Math.min(1.4 * bounds.readableScale, width / bounds.width, (height - 24) / bounds.height));
    camera.scale = mode === 'overview' ? overview : Math.max(bounds.readableScale, overview);
    camera.x = Math.max(0, (width - bounds.width * camera.scale) / 2);
    camera.y = Math.max(12, (height - bounds.height * camera.scale) / 2);
    render();
  };
  const zoom = (/** @type {number} */ factor, x = canvas.clientWidth / 2, y = canvas.clientHeight / 2) => {
    invariant(Number.isFinite(factor) && factor > 0, 'Zoom factor must be positive');
    invariant(Number.isFinite(x) && Number.isFinite(y), 'Zoom origin must be finite');
    const scale = Math.max(.02, Math.min(3 * bounds.readableScale, camera.scale * factor));
    camera.x = x - (x - camera.x) * scale / camera.scale;
    camera.y = y - (y - camera.y) * scale / camera.scale;
    camera.scale = scale; mode = 'manual'; render();
  };
  const pan = (/** @type {number} */ x, /** @type {number} */ y) => {
    invariant(Number.isFinite(x) && Number.isFinite(y), 'Pan distance must be finite');
    invariant(Number.isFinite(camera.x) && Number.isFinite(camera.y), 'Camera position must be finite');
    camera.x += x; camera.y += y; mode = 'manual'; render();
  };
  const release = gestures(canvas, zoom, pan);
  const releaseKeyboard = keyboardNavigation(canvas, zoom, pan, () => camera.scale, () => bounds.readableScale);
  const observer = new ResizeObserver(() => { if (mode !== 'manual') fit(); }); observer.observe(canvas);
  const controls = viewportControls(zoom, () => { mode = 'overview'; fit(); }, status);
  return { controls, update: (/** @type {{width:number,height:number,readableScale:number}} */ value) => {
    invariant([value.width, value.height, value.readableScale].every(number => Number.isFinite(number) && number > 0), 'Viewport bounds and readable scale must be positive');
    bounds = value; if (mode !== 'manual') fit(); else render();
  }, dispose: () => { observer.disconnect(); release(); releaseKeyboard(); } };
}

/** @param {(factor:number)=>void} zoom @param {()=>void} fit @param {HTMLElement} status */
function viewportControls(zoom, fit, status) {
  const controls = el('div', 'fleet-graph-controls');
  controls.setAttribute('role', 'group'); controls.setAttribute('aria-label', 'Graph navigation');
  const zoomControls = el('div', 'fleet-graph-zoom');
  const actions = [['Zoom out', '−', () => zoom(1 / 1.25)], ['Fit graph to screen', 'Fit', fit], ['Zoom in', '+', () => zoom(1.25)]];
  for (const [label, text, action] of /** @type {[string,string,()=>void][]} */ (actions)) {
    const control = button(text, action, 'button fleet-graph-control');
    control.setAttribute('aria-label', label); control.title = label; zoomControls.append(control);
  }
  invariant(zoomControls.children.length === 3, 'Graph requires zoom out, fit and zoom in');
  invariant(!status.parentElement, 'Viewport status must belong to one graph');
  controls.append(status, zoomControls);
  return controls;
}

/** @param {SVGSVGElement} canvas @param {(factor:number)=>void} zoom @param {(x:number,y:number)=>void} pan @param {()=>number} scale @param {()=>number} readableScale */
function keyboardNavigation(canvas, zoom, pan, scale, readableScale) {
  invariant(canvas.getAttribute('tabindex') === '0', 'Graph canvas must be keyboard reachable');
  invariant(canvas.hasAttribute('aria-label'), 'Graph keyboard navigation needs a label');
  const focus = (/** @type {FocusEvent} */ event) => {
    const target = event.target;
    if (!(target instanceof SVGElement) || !target.matches('.fleet-graph-node')) return;
    if (scale() < readableScale()) zoom(readableScale() / scale());
    const rect = target.getBoundingClientRect(); const area = canvas.getBoundingClientRect();
    if (rect.left < area.left || rect.right > area.right || rect.top < area.top || rect.bottom > area.bottom) {
      pan(area.left + area.width / 2 - rect.left - rect.width / 2, area.top + area.height / 2 - rect.top - rect.height / 2);
    }
  };
  const keydown = (/** @type {KeyboardEvent} */ event) => {
    if (event.altKey || event.ctrlKey || event.metaKey) return;
    const direction = /** @type {Record<string,number[]>} */ ({ ArrowLeft: [80, 0], ArrowRight: [-80, 0], ArrowUp: [0, 80], ArrowDown: [0, -80] })[event.key];
    if (!direction) return;
    event.preventDefault(); pan(direction[0], direction[1]);
  };
  canvas.addEventListener('focusin', focus); canvas.addEventListener('keydown', keydown);
  return () => { canvas.removeEventListener('focusin', focus); canvas.removeEventListener('keydown', keydown); };
}

/** @param {SVGSVGElement} canvas @param {(factor:number,x?:number,y?:number)=>void} zoom @param {(x:number,y:number)=>void} pan */
function gestures(canvas, zoom, pan) {
  /** @type {Map<number,{x:number,y:number}>} */ const pointers = new Map();
  let dragged = false;
  const down = (/** @type {PointerEvent} */ event) => {
    if (event.button !== 0) return;
    if (!pointers.size) dragged = false;
    pointers.set(event.pointerId, { x: event.clientX, y: event.clientY });
    const target = event.target instanceof Element ? event.target.closest('.fleet-graph-node') || canvas : canvas;
    target.setPointerCapture(event.pointerId);
  };
  const move = (/** @type {PointerEvent} */ event) => {
    const previous = pointers.get(event.pointerId); if (!previous) return;
    const next = { x: event.clientX, y: event.clientY };
    if (Math.hypot(next.x - previous.x, next.y - previous.y) < 2 && !dragged) return;
    dragged = true;
    const other = [...pointers].find(([id]) => id !== event.pointerId)?.[1];
    if (other) {
      const before = Math.hypot(previous.x - other.x, previous.y - other.y);
      const after = Math.hypot(next.x - other.x, next.y - other.y);
      const rect = canvas.getBoundingClientRect();
      if (before > 2 && after > 2) zoom(after / before, (next.x + other.x) / 2 - rect.left, (next.y + other.y) / 2 - rect.top);
    } else pan(next.x - previous.x, next.y - previous.y);
    pointers.set(event.pointerId, next);
  };
  const up = (/** @type {PointerEvent} */ event) => pointers.delete(event.pointerId);
  const click = (/** @type {MouseEvent} */ event) => {
    if (!dragged || event.detail === 0) return;
    event.preventDefault(); event.stopImmediatePropagation(); dragged = false;
  };
  const wheel = (/** @type {WheelEvent} */ event) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault(); const rect = canvas.getBoundingClientRect();
    zoom(Math.exp(-event.deltaY * .006), event.clientX - rect.left, event.clientY - rect.top);
  };
  canvas.addEventListener('pointerdown', down); canvas.addEventListener('pointermove', move);
  canvas.addEventListener('pointerup', up); canvas.addEventListener('pointercancel', up);
  canvas.addEventListener('click', click, true); canvas.addEventListener('wheel', wheel, { passive: false });
  return () => {
    canvas.removeEventListener('pointerdown', down); canvas.removeEventListener('pointermove', move);
    canvas.removeEventListener('pointerup', up); canvas.removeEventListener('pointercancel', up);
    canvas.removeEventListener('click', click, true); canvas.removeEventListener('wheel', wheel);
  };
}
