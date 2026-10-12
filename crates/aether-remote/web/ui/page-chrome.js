import {el,invariant} from './dom.js';

const entryFrames=Array.from({length:21},(_,index)=>{
  const time=index/20*9;
  // The critically damped spring settles without overshooting the reading position.
  const progress=index===20 ? 1 : 1-(1+time)*Math.exp(-time);
  return {opacity:progress,transform:`translateY(${(1-progress)*10}px)`,offset:index/20};
});

/** @param {HTMLElement} shell @param {HTMLElement} topbar @param {HTMLElement} main */
export function pageChrome(shell,topbar,main) {
  const title=el('span','compact-page-title');title.setAttribute('aria-hidden','true');
  topbar.prepend(title);
  /** @type {HTMLElement|null} */ let activeHeading=null;
  const observer=new IntersectionObserver(entries=>{
    const entry=entries[0];
    if (entry?.target===activeHeading) shell.classList.toggle('page-scrolled',(window.scrollY>0 || main.scrollTop>0) && entry.boundingClientRect.bottom<=24);
  },{rootMargin:'-24px 0px 0px 0px',threshold:0});
  return (/** @type {string} */ screen)=>{
    invariant(screen.length>0,'Page chrome requires a screen');
    invariant(main.isConnected,'Page chrome requires the mounted main');
    observer.disconnect();shell.classList.remove('page-scrolled');
    shell.dataset.screen=screen;main.dataset.screen=screen;
    const heading=main.querySelector('h1');
    activeHeading=heading;
    title.textContent=heading?.textContent || '';
    if (heading && screen!=='agent' && screen!=='pair') observer.observe(heading);
    const reduced=matchMedia('(prefers-reduced-motion: reduce)').matches && document.documentElement.dataset.motion!=='full';
    if (reduced || document.documentElement.dataset.motion==='reduced' || screen==='agent') return;
    main.getAnimations().forEach(animation=>animation.cancel());
    main.animate(entryFrames,{duration:350,easing:'linear'});
  };
}
