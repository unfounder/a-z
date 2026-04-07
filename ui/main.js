const { invoke } = window.__TAURI__.tauri;
const { WebviewWindow, appWindow, PhysicalPosition, PhysicalSize } = window.__TAURI__.window;
const tauriEvent = window.__TAURI__.event;

const urlInput    = document.getElementById('url-input');
const navigateBtn = document.getElementById('navigate-btn');
const content     = document.getElementById('content');
const statusText  = document.getElementById('status-text');
const statusBar   = document.querySelector('.status-bar');
const backBtn     = document.getElementById('back-btn');
const forwardBtn  = document.getElementById('forward-btn');
const refreshBtn  = document.getElementById('refresh-btn');
const stopBtn     = document.getElementById('stop-btn');
const homeBtn     = document.getElementById('home-btn');
const modeBtn     = document.getElementById('mode-btn');

let navHistory   = [];
let historyIndex = -1;
let loading      = false;
let viewport     = null;   // overlay WebviewWindow for hard sites
let viewportUrl  = '';     // current URL loaded in viewport
let usingViewport = false; // true when viewport is showing

// ── Hard-site detection ────────────────────────────────────────────────────────
const HARD_SITES = [
    'youtube.com', 'youtu.be',
    'amazon.com', 'amazon.in', 'amazon.co.uk', 'amazon.de', 'amazon.fr',
    'twitter.com', 'x.com',
    'facebook.com', 'fb.com', 'instagram.com',
    'netflix.com', 'twitch.tv', 'tiktok.com', 'spotify.com',
    'google.com', 'gmail.com', 'maps.google.com',
    'docs.google.com', 'sheets.google.com', 'drive.google.com',
    'reddit.com', 'discord.com', 'slack.com',
    'linkedin.com', 'pinterest.com', 'tumblr.com',
];

function isHardSite(hostname) {
    return HARD_SITES.some(d => hostname === d || hostname.endsWith('.' + d));
}

// ── Helpers ────────────────────────────────────────────────────────────────────
function setStatus(msg) { statusText.textContent = msg; }

function setLoading(on) {
    loading = on;
    navigateBtn.disabled = on;
    stopBtn.disabled = !on;
    statusBar.classList.toggle('loading', on);
    if (!on) setStatus('Done');
}

function updateNavButtons() {
    backBtn.disabled    = historyIndex <= 0;
    forwardBtn.disabled = historyIndex >= navHistory.length - 1;
}

// ── Viewport: overlay WebviewWindow over the content area ──────────────────────
async function getContentPhysicalRect() {
    const el   = document.getElementById('content-wrap');
    const rect = el.getBoundingClientRect();
    const pos  = await appWindow.outerPosition();
    const sf   = await appWindow.scaleFactor();
    return {
        x: pos.x + Math.round(rect.left * sf),
        y: pos.y + Math.round(rect.top  * sf),
        w: Math.round(rect.width  * sf),
        h: Math.round(rect.height * sf),
    };
}

async function syncViewport() {
    if (!viewport) return;
    try {
        const r = await getContentPhysicalRect();
        await viewport.setPosition(new PhysicalPosition(r.x, r.y));
        await viewport.setSize(new PhysicalSize(r.w, r.h));
    } catch (_) {}
}

async function ensureViewport(url) {
    const r = await getContentPhysicalRect();

    if (!viewport) {
        const existing = WebviewWindow.getByLabel('bose-vp');
        if (existing) {
            viewport = existing;
        } else {
            viewport = new WebviewWindow('bose-vp', {
                url,
                decorations: false,
                skipTaskbar:  true,
                alwaysOnTop:  false,
                resizable:    false,
                x: r.x, y: r.y,
                width: r.w, height: r.h,
            });
            await new Promise(resolve => {
                viewport.once('tauri://created', resolve);
                setTimeout(resolve, 4000);
            });
        }
        // Keep viewport in sync when main window moves/resizes
        appWindow.onMoved(syncViewport);
        appWindow.onResized(syncViewport);
    } else {
        await syncViewport();
        await viewport.show();
        await viewport.navigate(url);
    }
    usingViewport = true;
    viewportUrl   = url;
}

async function hideViewport() {
    if (viewport) {
        try { await viewport.hide(); } catch (_) {}
    }
    usingViewport = false;
}

// ── Welcome page ───────────────────────────────────────────────────────────────
function showWelcome() {
    hideViewport();
    content.innerHTML = `
        <div id="welcome">
            <div class="welcome-logo">A–Z</div>
            <div class="welcome-tagline">Your Personal Internet Explorer</div>
            <div class="welcome-divider"></div>
            <div class="welcome-engine-info">
                <p>
                    <strong>Bose mode</strong> — Rust fetch → html5ever → QuickJS → serialize (fast, static sites)<br>
                    <strong>Hard sites</strong> (YouTube, Amazon…) — full WebView2 embedded in this window
                </p>
            </div>
            <div class="welcome-links">
                <a href="#" onclick="loadUrl('https://wikipedia.org')">📖 Wikipedia</a>
                <a href="#" onclick="loadUrl('https://news.ycombinator.com')">🔥 Hacker News</a>
                <a href="#" onclick="loadUrl('https://youtube.com')">▶ YouTube</a>
                <a href="#" onclick="loadUrl('https://amazon.com')">🛒 Amazon</a>
                <a href="#" onclick="loadUrl('https://example.com')">🌐 Example</a>
            </div>
        </div>`;
    urlInput.value = '';
    setStatus('Ready');
    updateNavButtons();
}

// ── Mode toggle ────────────────────────────────────────────────────────────────
let directMode = false;
modeBtn.addEventListener('click', () => {
    directMode = !directMode;
    if (directMode) {
        modeBtn.textContent = '⚡ Direct';
        modeBtn.className = 'mode-btn direct-mode';
        setStatus('Direct mode — all sites use WebView2');
    } else {
        modeBtn.textContent = '🌿 Bose';
        modeBtn.className = 'mode-btn engine-mode';
        setStatus('Bose mode — Rust engine for normal sites');
    }
});

// ── Navigation ─────────────────────────────────────────────────────────────────
async function navigate(rawUrl, pushHistory = true) {
    if (!rawUrl) return;

    let url = rawUrl.trim();
    if (!url.startsWith('http://') && !url.startsWith('https://')) {
        // If it looks like a search query, send to DuckDuckGo
        if (!url.includes('.') || url.includes(' ')) {
            url = 'https://duckduckgo.com/?q=' + encodeURIComponent(url);
        } else {
            url = 'https://' + url;
        }
    }
    urlInput.value = url;
    setLoading(true);

    let hostname = '';
    try { hostname = new URL(url).hostname; } catch (_) {}

    const useViewport = directMode || isHardSite(hostname);

    try {
        if (useViewport) {
            // ── Hard site / Direct mode: full WebView2 in overlay window ──────
            setStatus('Loading ' + hostname + '…');
            content.innerHTML = `
                <div style="
                    width:100%;height:100%;
                    display:flex;flex-direction:column;align-items:center;justify-content:center;
                    background:var(--cream);gap:12px;color:var(--text2);font-size:13px;
                ">
                    <div style="font-size:32px">🌿</div>
                    <div style="font-weight:600">Opening <b>${hostname}</b></div>
                    <div style="font-size:11px;opacity:0.6">WebView2 — full JavaScript, cookies, everything</div>
                </div>`;
            await ensureViewport(url);

        } else {
            // ── Bose engine: Rust fetch + html5ever + QuickJS ─────────────────
            await hideViewport();
            setStatus('Bose: connecting to ' + hostname + '…');
            const html = await invoke('navigate', { url });
            setStatus('Rendering ' + hostname + '…');

            const iframe = document.createElement('iframe');
            iframe.style.cssText = 'width:100%;height:100%;border:none;display:block;';
            iframe.srcdoc = html;
            content.innerHTML = '';
            content.appendChild(iframe);

            // Intercept all link clicks and form submits → route through Bose
            iframe.onload = () => {
                try {
                    const iDoc = iframe.contentDocument;
                    if (!iDoc) return;
                    iDoc.addEventListener('click', (e) => {
                        const a = e.target.closest('a[href]');
                        if (!a) return;
                        const href = a.getAttribute('href');
                        if (!href || href.startsWith('#') || href.startsWith('javascript')) return;
                        e.preventDefault();
                        const base = url;
                        let full;
                        try {
                            full = new URL(href, base).href;
                        } catch (_) {
                            full = href.startsWith('//') ? 'https:' + href : href;
                        }
                        navigate(full);
                    });
                    iDoc.addEventListener('submit', (e) => {
                        e.preventDefault();
                        const form = e.target;
                        const params = new URLSearchParams(new FormData(form)).toString();
                        const action = form.action || url;
                        navigate(action + (params ? (action.includes('?') ? '&' : '?') + params : ''));
                    });
                } catch (_) {}
            };
        }

        if (pushHistory) {
            navHistory = navHistory.slice(0, historyIndex + 1);
            navHistory.push(url);
            historyIndex = navHistory.length - 1;
        }
        updateNavButtons();

    } catch (error) {
        await hideViewport();
        const errMsg = String(error);
        content.innerHTML = `
            <div class="error-page">
                <h2>Cannot open this page</h2>
                <p>The Bose engine could not load this page.</p>
                <hr style="margin:16px 0;border:none;border-top:1px solid var(--mist)">
                <p><strong>Details:</strong><br><code>${errMsg}</code></p>
                <p style="margin-top:16px">
                    <a href="#" onclick="loadUrlDirect('${url.replace(/'/g, "\\'")}')">
                        Open with full WebView2 →
                    </a>
                </p>
            </div>`;
        setStatus('Error loading ' + hostname);
    } finally {
        setLoading(false);
    }
}

// ── Public helpers ─────────────────────────────────────────────────────────────
window.loadUrl = function(url) { navigate(url); };
window.loadUrlDirect = function(url) {
    // Force WebView2 for this URL regardless of mode
    const prev = directMode;
    directMode = true;
    navigate(url).finally(() => { directMode = prev; });
};

// ── Toolbar buttons ────────────────────────────────────────────────────────────
navigateBtn.addEventListener('click', () => navigate(urlInput.value));
urlInput.addEventListener('keydown', e => { if (e.key === 'Enter') navigate(urlInput.value); });

backBtn.addEventListener('click', () => {
    if (historyIndex > 0) {
        historyIndex--;
        navigate(navHistory[historyIndex], false);
    }
});
forwardBtn.addEventListener('click', () => {
    if (historyIndex < navHistory.length - 1) {
        historyIndex++;
        navigate(navHistory[historyIndex], false);
    }
});
refreshBtn.addEventListener('click', () => {
    if (usingViewport && viewport) {
        viewport.navigate(viewportUrl).catch(() => {});
    } else if (urlInput.value) {
        navigate(urlInput.value, false);
    }
});
stopBtn.addEventListener('click', () => { setLoading(false); setStatus('Stopped'); });
homeBtn.addEventListener('click', showWelcome);

// ── Init ───────────────────────────────────────────────────────────────────────
setLoading(false);
stopBtn.disabled = true;
updateNavButtons();
