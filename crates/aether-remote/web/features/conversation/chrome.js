import {button, invariant} from '../../ui/dom.js';

const paths = {
  back:'m14 5-7 7 7 7', more:'M5 12h.01M12 12h.01M19 12h.01',
  send:'M12 19V5m-6 6 6-6 6 6', close:'m6 6 12 12M6 18 18 6',
  previous:'m6 15 6-6 6 6', next:'m6 9 6 6 6-6',
};

/** @param {keyof typeof paths} name @param {string} label @param {()=>void} action */
export function chatButton(name, label, action) {
  invariant(name in paths, 'Chat icon must exist');
  invariant(label.length > 0, 'Icon buttons require an accessible name');
  const node = button('', action, 'button conversation-icon');
  node.setAttribute('aria-label', label);
  node.title = label;
  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 24 24');
  svg.setAttribute('fill', 'none');
  svg.setAttribute('stroke', 'currentColor');
  svg.setAttribute('stroke-width', name === 'more' ? '3.5' : '1.8');
  svg.setAttribute('stroke-linecap', 'round');
  svg.setAttribute('stroke-linejoin', 'round');
  svg.setAttribute('aria-hidden', 'true');
  const path = document.createElementNS(svg.namespaceURI, 'path');
  path.setAttribute('d', paths[name]);
  svg.append(path); node.append(svg);
  return node;
}

/** @param {HTMLElement} root @param {HTMLElement} composer */
export function keyboardDock(root, composer) {
  invariant(root instanceof HTMLElement, 'Keyboard dock requires a screen');
  invariant(composer.parentElement === root, 'Composer must belong to the screen');
  const viewport = window.visualViewport;
  let frame = 0;
  const update = () => {
    frame = 0;
    if (!viewport || viewport.scale !== 1) return;
    root.style.setProperty('--conversation-height', `${viewport.height}px`);
    root.style.setProperty('--conversation-top', `${viewport.offsetTop}px`);
    root.dataset.keyboard = String(document.documentElement.clientHeight - viewport.height > 120);
  };
  const schedule = () => { if (!frame) frame = requestAnimationFrame(update); };
  const observer = new ResizeObserver(() => {
    root.style.setProperty('--composer-height', `${composer.getBoundingClientRect().height}px`);
  });
  observer.observe(composer);
  viewport?.addEventListener('resize', schedule);
  viewport?.addEventListener('scroll', schedule);
  window.addEventListener('resize', schedule);
  update();
  return () => {
    cancelAnimationFrame(frame); observer.disconnect();
    viewport?.removeEventListener('resize', schedule);
    viewport?.removeEventListener('scroll', schedule);
    window.removeEventListener('resize', schedule);
  };
}
