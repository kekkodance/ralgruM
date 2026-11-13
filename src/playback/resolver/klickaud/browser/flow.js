(async () => {
  'use strict';

  // Wry injects initialization scripts into every frame on Windows, even when
  // main-only injection is requested. Only the top frame may drive the flow.
  if (window.top !== window) return;
  if (window.__kaFlowStarted) return;
  window.__kaFlowStarted = true;

  const TRACK = __TRACK_URL__;
  const PREVIEW = __PREVIEW__;
  const PREVIEW_ONLY = __PREVIEW_ONLY__;
  if (
    !PREVIEW_ONLY &&
    location.hostname !== 'www.klickaud.org' &&
    location.hostname !== 'klickaud.org'
  ) {
    return;
  }
  const CANCELLED = 'Playback request cancelled';
  const post = (payload) => {
    try {
      window.ipc.postMessage(JSON.stringify(payload));
    } catch (_) {}
  };
  const session = { cancelled: false, challengeReject: null };
  const abort = new AbortController();

  window.__kaCancel = () => {
    if (session.cancelled) return;
    session.cancelled = true;
    try {
      abort.abort();
    } catch (_) {}
    if (session.challengeReject) {
      const reject = session.challengeReject;
      session.challengeReject = null;
      try {
        reject(new Error(CANCELLED));
      } catch (_) {}
    }
  };

  const ensureLive = () => {
    if (session.cancelled) throw new Error(CANCELLED);
  };

  const sha256hex = async (value) => {
    const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value));
    return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
  };

  const isKlickaudHost = (host) => host === 'klickaud.org' || host.endsWith('.klickaud.org');

  const absoluteDownloadUrl = (value) => {
    const parsed = new URL(value, location.href);
    if (parsed.protocol !== 'https:') {
      throw new Error('KlickAud returned a non-HTTPS download URL.');
    }
    if (!isKlickaudHost(parsed.hostname)) {
      throw new Error('KlickAud returned a download URL on an unexpected host.');
    }
    return parsed.toString();
  };

  let host;
  let overlay;
  const isolate = () => {
    if (!host || !overlay) return;
    for (const child of Array.from(document.body.children)) {
      if (child !== host) child.style.display = 'none';
    }
    host.style.setProperty('display', 'block', 'important');
    overlay.style.display = 'flex';
  };

  try {
    if (document.readyState === 'loading') {
      await new Promise((resolve) => {
        document.addEventListener('DOMContentLoaded', resolve, { once: true });
      });
    }
    ensureLive();

    document.documentElement.style.background = '#09090b';
    // Keep the host page dark and unscrollable. The overlay itself lives in a
    // shadow root so the KlickAud stylesheet cannot restyle its controls.
    const pageStyle = document.createElement('style');
    pageStyle.textContent =
      'html,body{margin:0;width:100%;height:100%;overflow:hidden;background:#09090b;}';
    document.head.appendChild(pageStyle);

    host = document.createElement('div');
    host.id = 'ka-verification-host';
    // Inline important declarations outrank any KlickAud rule that could
    // otherwise re-style or re-position the host element itself.
    host.style.cssText =
      'all:initial !important;position:fixed !important;inset:0 !important;' +
      'z-index:2147483647 !important;display:none !important;';
    const root = host.attachShadow({ mode: 'open' });
    const style = document.createElement('style');
    style.textContent =
      '#ka-verification-overlay{position:fixed;inset:0;z-index:2147483647;display:none;' +
      'flex-direction:column;background:#09090b;color:#fafafa;' +
      'user-select:none;-webkit-user-select:none;cursor:default;' +
      'font-family:"Segoe UI",system-ui,-apple-system,sans-serif;}' +
      '.ka-titlebar{height:32px;flex:0 0 32px;display:flex;align-items:center;' +
      'justify-content:space-between;background:#0c0c0e;border-bottom:1px solid #27272a;' +
      'box-sizing:border-box;}' +
      '.ka-titlebar-left{height:100%;flex:1;display:flex;align-items:center;gap:7px;' +
      'padding:0 11px;min-width:0;}' +
      '.ka-titlebar-icon{display:flex;width:14px;height:14px;flex:none;position:relative;' +
      'top:-1px;color:#6366f1;}' +
      '.ka-titlebar-icon svg{display:block;width:14px;height:14px;overflow:visible;}' +
      '.ka-titlebar-app{font-size:11.5px;font-weight:500;color:#d4d4d8;' +
      'white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}' +
      '.ka-window-controls{height:100%;flex:none;display:flex;}' +
      '.ka-window-control{width:46px;height:32px;flex:none;display:flex;align-items:center;' +
      'justify-content:center;margin:0;padding:0;border:0;background:transparent;' +
      'color:#a1a1aa;font:inherit;}' +
      '.ka-window-control:hover{background:#18181b;color:#fafafa;}' +
      '.ka-window-control.ka-close:hover{background:#c42b1c;color:#ffffff;}' +
      '.ka-min svg{display:block;width:11px;height:12.571px;overflow:visible;}' +
      '.ka-close svg{display:block;width:10.5px;height:14px;overflow:visible;}' +
      '.ka-body{flex:1;display:flex;align-items:center;justify-content:center;padding:24px;' +
      'box-sizing:border-box;min-height:0;}' +
      '.ka-card{width:100%;max-width:420px;box-sizing:border-box;padding:28px 32px;' +
      'text-align:center;background:#121215;border:1px solid #27272a;border-radius:6px;}' +
      '.ka-heading{font-size:16px;font-weight:600;color:#fafafa;}' +
      '.ka-copy{margin-top:8px;font-size:13px;line-height:1.5;color:#a1a1aa;}' +
      '.ka-slot{margin-top:22px;display:flex;justify-content:center;min-height:65px;}' +
      '.ka-status{margin-top:14px;font-size:12px;color:#a1a1aa;min-height:18px;}';
    root.appendChild(style);

    overlay = document.createElement('div');
    overlay.id = 'ka-verification-overlay';
    overlay.innerHTML =
      '<div class="ka-titlebar" id="ka-titlebar">' +
      '<div class="ka-titlebar-left">' +
      '<span class="ka-titlebar-icon" aria-hidden="true">' + __ROBOT_ICON__ + '</span>' +
      '<span class="ka-titlebar-app">KlickAud Verification</span>' +
      '</div>' +
      '<div class="ka-window-controls">' +
      '<button type="button" class="ka-window-control ka-min" id="ka-min" ' +
      'aria-label="Minimize" title="Minimize">' + __MINUS_ICON__ + '</button>' +
      '<button type="button" class="ka-window-control ka-close" id="ka-close" ' +
      'aria-label="Close" title="Close">' + __XMARK_ICON__ + '</button>' +
      '</div>' +
      '</div>' +
      '<div class="ka-body">' +
      '<div class="ka-card">' +
      '<div class="ka-heading">Verifying your browser</div>' +
      '<div class="ka-copy">KlickAud is checking this device before preparing the download.</div>' +
      '<div id="ka-turnstile-slot" class="ka-slot"></div>' +
      '<div id="ka-turnstile-status" class="ka-status" role="status">' +
      'This can take a few seconds.' +
      '</div>' +
      '</div>' +
      '</div>';
    root.appendChild(overlay);
    document.body.appendChild(host);

    overlay.querySelector('#ka-close').addEventListener('click', (event) => {
      event.preventDefault();
      post({ type: 'close' });
    });
    overlay.querySelector('#ka-min').addEventListener('click', (event) => {
      event.preventDefault();
      post({ type: 'minimize' });
    });
    // GPUI sizes titlebar SVGs by width and keeps their viewBox aspect ratio,
    // so give these their natural height instead of squashing them square.
    overlay.querySelector('.ka-titlebar').addEventListener('pointerdown', (event) => {
      if (event.button !== 0) return;
      if (event.target && event.target.closest && event.target.closest('.ka-window-control')) {
        return;
      }
      event.preventDefault();
      post({ type: 'drag' });
    });
    const slot = overlay.querySelector('#ka-turnstile-slot');
    const status = overlay.querySelector('#ka-turnstile-status');
    const setStatus = (text) => {
      if (status) status.textContent = text;
    };

    if (PREVIEW) {
      // Preview mode keeps the window visible so the styling can be checked
      // even when Cloudflare solves the challenge without interaction.
      isolate();
      setStatus('Preview mode. Close this window when you are done.');
      post({ type: 'preview' });
    }
    if (PREVIEW_ONLY) return;

    post({ type: 'progress', step: 'csrf' });
    // The landing page bootstraps the form token itself. Fetching a second
    // token here races with that bootstrap and can invalidate the token that
    // the page just installed, so use the page's token whenever it exists.
    const csrfInput = document.getElementById('csrf_token');
    let csrfToken = '';
    if (csrfInput) {
      const deadline = Date.now() + 5000;
      while (Date.now() < deadline) {
        ensureLive();
        csrfToken = typeof csrfInput.value === 'string' ? csrfInput.value.trim() : '';
        if (csrfToken) break;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
    }
    if (!csrfToken) {
      const csrfResponse = await fetch('/csrf-token-endpoint.php', {
        credentials: 'same-origin',
        cache: 'no-store',
        signal: abort.signal,
        headers: { Accept: 'application/json' },
      });
      ensureLive();
      if (!csrfResponse.ok) {
        throw new Error('KlickAud CSRF request failed (' + csrfResponse.status + ').');
      }
      const csrf = await csrfResponse.json();
      if (!csrf || typeof csrf.csrf_token !== 'string' || csrf.csrf_token.length === 0) {
        throw new Error('KlickAud did not provide a CSRF token.');
      }
      csrfToken = csrf.csrf_token;
    }

    post({ type: 'progress', step: 'grant' });
    const downloadForm = csrfInput && csrfInput.form ? csrfInput.form : null;
    const downloadEndpoint =
      downloadForm && downloadForm.action
        ? downloadForm.action
        : new URL('/download.php', location.href).toString();
    const form = new URLSearchParams({ value: TRACK, csrf_token: csrfToken });
    const downloadResponse = await fetch(downloadEndpoint, {
      method: 'POST',
      credentials: 'same-origin',
      cache: 'no-store',
      signal: abort.signal,
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
      body: form.toString(),
    });
    ensureLive();
    const html = await downloadResponse.text();
    if (!downloadResponse.ok) {
      throw new Error('KlickAud download request failed (' + downloadResponse.status + ').');
    }

    const grant = /sseGrant\s*=\s*["']([^"']+)["']/.exec(html)?.[1];
    const mode = /downloadMode\s*=\s*["']([^"']+)["']/.exec(html)?.[1];
    const direct = /directDownloadUrl\s*=\s*["']([^"']*)["']/.exec(html)?.[1];
    if (!grant) {
      const title = /<title[^>]*>([^<]*)<\/title>/i.exec(html)?.[1]?.trim();
      throw new Error(
        'KlickAud did not provide a download grant (' +
          'status=' + downloadResponse.status +
          ', mode=' + (mode || 'unknown') +
          ', bytes=' + html.length +
          ', url=' + downloadResponse.url +
          ', title=' + (title || 'none') +
          ').'
      );
    }
    if (mode === 'direct' && direct) {
      post({ type: 'done', url: absoluteDownloadUrl(direct) });
      return;
    }
    if (mode !== 'worker') {
      throw new Error('KlickAud returned an unsupported download mode: ' + (mode || 'unknown') + '.');
    }

    post({ type: 'progress', step: 'turnstile' });
    if (!window.turnstile) {
      await new Promise((resolve, reject) => {
        const script = document.createElement('script');
        script.src = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
        script.async = true;
        const timer = setTimeout(() => reject(new Error('The verification library timed out.')), 20000);
        script.onload = () => {
          clearTimeout(timer);
          resolve();
        };
        script.onerror = () => {
          clearTimeout(timer);
          reject(new Error('The verification library failed to load.'));
        };
        document.head.appendChild(script);
      });
    }
    ensureLive();

    const cData = await sha256hex(grant);
    setStatus('Waiting for Cloudflare verification.');
    let revealed = false;
    const token = await new Promise((resolve, reject) => {
      const box = document.createElement('div');
      slot.appendChild(box);
      let done = false;
      let widgetId;
      let timer;
      let revealTimer;

      const finish = (value, error) => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        clearTimeout(revealTimer);
        session.challengeReject = null;
        if (widgetId !== undefined) {
          try {
            window.turnstile.remove(widgetId);
          } catch (_) {}
        }
        box.remove();
        if (error) {
          reject(error instanceof Error ? error : new Error(String(error)));
        } else {
          resolve(value);
        }
      };

      session.challengeReject = (error) => finish(null, error);
      timer = setTimeout(() => finish(null, 'Verification timed out.'), 120000);
      // Cloudflare can decide to show an interactive challenge without
      // invoking before-interactive-callback while the host window is hidden.
      // Reveal the window if the widget has not produced a token after a
      // grace period, so the challenge can always be completed.
      revealTimer = setTimeout(() => {
        if (done || revealed) return;
        revealed = true;
        isolate();
        setStatus('Complete the verification to continue.');
        post({ type: 'interactive' });
      }, 8000);

      try {
        widgetId = window.turnstile.render(box, {
          sitekey: '0x4AAAAAAF88suoEAAkjvgsr',
          action: 'klickaud_prepare',
          cData,
          execution: 'execute',
          appearance: 'interaction-only',
          theme: 'dark',
          callback: (value) => finish(value),
          'error-callback': (code) => finish(null, 'Verification failed (' + code + ').'),
          'expired-callback': () => finish(null, 'Verification expired.'),
          'timeout-callback': () => finish(null, 'Verification timed out.'),
          'before-interactive-callback': () => {
            if (revealed) return;
            revealed = true;
            isolate();
            post({ type: 'interactive' });
          },
          'after-interactive-callback': () => {},
        });
        window.turnstile.execute(widgetId);
      } catch (_) {
        finish(null, 'Verification could not start.');
      }
    });
    ensureLive();
    post({ type: 'verified' });
    setStatus('Preparing the download.');

    post({ type: 'progress', step: 'capability' });
    const capabilityResponse = await fetch('/sse_capability.php', {
      method: 'POST',
      credentials: 'same-origin',
      cache: 'no-store',
      signal: abort.signal,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ grant, url: TRACK, turnstile: token }),
    });
    ensureLive();
    const capabilityText = await capabilityResponse.text();
    let capability = null;
    try {
      capability = JSON.parse(capabilityText);
    } catch (_) {}
    if (
      !capabilityResponse.ok ||
      !capability ||
      capability.ok !== true ||
      typeof capability.capability !== 'string' ||
      capability.capability.length === 0
    ) {
      const reason =
        capability && typeof capability.error === 'string'
          ? ', reason=' + capability.error
          : '';
      throw new Error(
        'KlickAud capability request failed (' + capabilityResponse.status + reason + ').'
      );
    }

    post({ type: 'progress', step: 'worker' });
    const workerUrl =
      '/worker_sse.php?url=' +
      encodeURIComponent(TRACK) +
      '&cap=' +
      encodeURIComponent(capability.capability);
    const workerResponse = await fetch(workerUrl, {
      credentials: 'same-origin',
      cache: 'no-store',
      signal: abort.signal,
      headers: { Accept: 'text/event-stream' },
    });
    ensureLive();
    if (!workerResponse.ok) {
      throw new Error('KlickAud worker request failed (' + workerResponse.status + ').');
    }
    if (!workerResponse.body) {
      throw new Error('KlickAud worker stream was unavailable.');
    }

    const reader = workerResponse.body.getReader();
    const decoder = new TextDecoder();
    let pending = '';
    let eventName = '';
    let dataLines = [];

    const dispatch = () => {
      if (!eventName && dataLines.length === 0) return null;
      const name = eventName;
      const data = dataLines.join('\n');
      eventName = '';
      dataLines = [];
      if (name === 'ready') {
        try {
          const parsed = JSON.parse(data);
          if (parsed && typeof parsed.download_url === 'string' && parsed.download_url.length > 0) {
            return { url: absoluteDownloadUrl(parsed.download_url) };
          }
          return { error: 'KlickAud ready event had no download URL.' };
        } catch (_) {
          return { error: 'KlickAud ready event was not valid JSON.' };
        }
      }
      if (name === 'failed') {
        let message = 'KlickAud worker reported a failure.';
        try {
          const parsed = JSON.parse(data);
          if (parsed && typeof parsed.message === 'string' && parsed.message.length > 0) {
            message += ' ' + parsed.message;
          }
        } catch (_) {}
        return { error: message };
      }
      return null;
    };

    while (true) {
      const chunk = await reader.read();
      ensureLive();
      if (chunk.done) break;
      pending += decoder.decode(chunk.value, { stream: true });
      let newline;
      while ((newline = pending.indexOf('\n')) >= 0) {
        let line = pending.slice(0, newline);
        pending = pending.slice(newline + 1);
        if (line.endsWith('\r')) line = line.slice(0, -1);
        if (line === '') {
          const outcome = dispatch();
          if (outcome && outcome.url) {
            try {
              await reader.cancel();
            } catch (_) {}
            post({ type: 'done', url: outcome.url });
            return;
          }
          if (outcome && outcome.error) throw new Error(outcome.error);
          continue;
        }
        if (line.startsWith('event:')) {
          eventName = line.slice(6).trim();
        } else if (line.startsWith('data:')) {
          dataLines.push(line.slice(5).replace(/^ /, ''));
        }
      }
    }
    throw new Error('KlickAud worker ended without a download URL.');
  } catch (error) {
    const message =
      error && error.name === 'AbortError'
        ? CANCELLED
        : String(error && error.message ? error.message : error);
    if (message !== CANCELLED) {
      post({ type: 'error', message });
    }
  }
})();
