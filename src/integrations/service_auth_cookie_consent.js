(() => {
    const knownRejectSelectors = [
        '#onetrust-reject-all-handler',
        '#didomi-notice-disagree-button',
        '#uc-btn-deny-banner',
        '[data-testid="uc-deny-all-button"]',
        '[data-testid="cookie-consent-reject-all"]',
        '[data-testid="consent-reject-all"]',
        '[aria-label="Reject all cookies"]',
        '[aria-label="Refuse all cookies"]'
    ];
    const rejectLabels = new Set([
        'reject all',
        'reject all cookies',
        'refuse all',
        'refuse all cookies',
        'decline all',
        'only necessary',
        'only necessary cookies',
        'continue without accepting',
        'rifiuta tutto',
        'rifiuta tutti',
        'solo necessari',
        'solo cookie necessari',
        'continua senza accettare',
        'tout refuser',
        'alle ablehnen',
        'nur notwendige',
        'rechazar todo',
        'solo necesarias',
        'rejeitar tudo',
        'apenas necessários',
        'alles weigeren',
        'alleen noodzakelijk',
        'odrzuć wszystkie'
    ]);
    let observer;
    let timer;
    let attempts = 0;

    const stop = () => {
        observer?.disconnect();
        if (timer) clearInterval(timer);
    };
    const visible = (element) =>
        element instanceof HTMLElement
        && !element.hasAttribute('disabled')
        && element.getClientRects().length > 0;
    const normalizedLabel = (element) =>
        (element.innerText || element.textContent || element.getAttribute('aria-label') || '')
            .replace(/\s+/g, ' ')
            .trim()
            .toLocaleLowerCase();
    const isRejectLabel = (element) => {
        const label = normalizedLabel(element);
        return rejectLabels.has(label)
            || (location.hostname.endsWith('deezer.com') && label === 'refuse');
    };
    const rejectConsent = () => {
        attempts += 1;
        const knownButton = document.querySelector(knownRejectSelectors.join(','));
        const textButton = [...document.querySelectorAll('button, [role="button"], input[type="button"]')]
            .find(isRejectLabel);
        const button = visible(knownButton) ? knownButton : visible(textButton) ? textButton : null;
        if (button) {
            button.click();
            stop();
            return;
        }
        if (attempts >= 120) stop();
    };
    const start = () => {
        if (!document.documentElement) {
            setTimeout(start, 50);
            return;
        }
        observer = new MutationObserver(rejectConsent);
        observer.observe(document.documentElement, { childList: true, subtree: true });
        timer = setInterval(rejectConsent, 500);
        rejectConsent();
        addEventListener('pagehide', stop, { once: true });
    };
    start();
})();
