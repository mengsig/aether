import('./app.js').catch(error => {
  const root = document.getElementById('app');
  if (!root) throw error;
  const main = document.createElement('main'); main.className = 'boot';
  const title = document.createElement('h1'); title.textContent = 'Couldn’t open Aether';
  const message = document.createElement('p');
  message.textContent = 'Your session could not start. Check your connection to the host, then try again.';
  const retry = document.createElement('button'); retry.type = 'button'; retry.className = 'button primary';
  retry.textContent = 'Reload Aether'; retry.addEventListener('click',()=>location.reload());
  const details = document.createElement('details');
  const summary = document.createElement('summary'); summary.textContent = 'Connection details';
  const reason = document.createElement('p'); reason.textContent = error instanceof Error ? error.message : String(error);
  details.append(summary,reason); main.append(title,message,retry,details); root.replaceChildren(main);
});
