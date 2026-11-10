import {errorText} from './dom.js';

/** @param {(message:string)=>void} report */
export function readPreferences(report) {
  const preferences = {theme:'system', motion:'system'};
  try {
    const saved = JSON.parse(localStorage.getItem('aether.preferences') || '{}');
    if (['system','light','dark'].includes(saved.theme)) preferences.theme = saved.theme;
    if (['system','reduced','full'].includes(saved.motion)) preferences.motion = saved.motion;
  } catch (error) { report(`Preferences could not be restored: ${errorText(error)}`); }
  applyPreferences(preferences);
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change',() => {
    if (preferences.theme === 'system') applyPreferences(preferences);
  });
  return preferences;
}

/** @param {{theme:string,motion:string}} preferences */
export function applyPreferences(preferences) {
  document.documentElement.dataset.theme = preferences.theme;
  document.documentElement.dataset.motion = preferences.motion;
  const dark = preferences.theme === 'dark' || (preferences.theme === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', dark ? '#0d1216' : '#f6f7f9');
}

/** @param {{theme:string,motion:string}} preferences @param {'theme'|'motion'} name @param {string} value */
export function savePreference(preferences, name, value) {
  const values = name === 'theme' ? ['system','light','dark'] : ['system','reduced','full'];
  if (!values.includes(value)) throw new Error(`Invalid ${name} preference`);
  preferences[name] = value;
  applyPreferences(preferences);
  localStorage.setItem('aether.preferences', JSON.stringify(preferences));
}
