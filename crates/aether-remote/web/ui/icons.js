const paths = {
  fleet: ['M12 3v5M5 16v-4h14v4', 'M9 3h6v5H9zM2 16h6v5H2zM16 16h6v5h-6z'],
  attention: ['M12 8v5m0 3h.01', 'M9.5 3h5L22 20H2z'],
  activity: ['M3 16l5-5 4 3 8-10', 'M3 3v18h18'],
  settings: ['M4 7h16M4 17h16', 'M8 4v6m8 4v6'],
  plus: ['M12 5v14M5 12h14'],
  arrow: ['m9 5 7 7-7 7'],
  folder: ['M3 7V5h6l2 2h10v13H3z'],
};
/** @param {keyof typeof paths} name */
export function icon(name) {
  const node = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  node.setAttribute('viewBox', '0 0 24 24');
  node.setAttribute('width', '22'); node.setAttribute('height', '22');
  node.setAttribute('fill', 'none'); node.setAttribute('stroke', 'currentColor');
  node.setAttribute('stroke-width', '1.65'); node.setAttribute('stroke-linecap', 'round');
  node.setAttribute('stroke-linejoin', 'round'); node.setAttribute('aria-hidden', 'true');
  for (const data of paths[name]) {
    const path = document.createElementNS(node.namespaceURI, 'path');
    path.setAttribute('d', data); node.append(path);
  }
  return node;
}
