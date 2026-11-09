// FizmatGuard: скрипт, который оболочка вставляет в fizmat.ai до кода страницы.
// Перед отправкой вопроса спрашивает локальную прослойку, что в нём личного,
// показывает окно «что скроем» и шлёт текст с метками; метки в ленте меняет обратно.
// Оболочка задаёт window.__FZ_GUARD = { url: 'http://127.0.0.1:47821', token: '…' }.
(() => {
    if (window.__fzGuardOn || !/(^|\.)fizmat\.ai$|^127\.0\.0\.1$|^localhost$/.test(location.hostname)) return;
    window.__fzGuardOn = true;
    const CFG = window.__FZ_GUARD || {};
    const TAG = /\[(ИМЯ|ПОЧТА|ТЕЛЕФОН|АДРЕС|ССЫЛКА|ДАТА|НОМЕР|СЕКРЕТ|ДАННЫЕ|СНИЛС|ИНН|КАРТА|ПАСПОРТ)_\d+\]/;
    const SEND = /^\/c\/[^/]+\/(messages|queue)$/;
    const KIND = { private_person: 'имя', private_email: 'почта', email: 'почта', private_phone: 'телефон', phone: 'телефон',
        private_address: 'адрес', private_url: 'ссылка', private_date: 'дата', account_number: 'номер', secret: 'секрет',
        other_pii: 'данные', snils: 'СНИЛС', inn: 'ИНН', card: 'карта', passport: 'паспорт', known: 'уже скрывали' };

    // В оболочке Tauri — через её команду (ключ у программы, не у страницы);
    // в пробе на обычной странице — прямо в сервис на 127.0.0.1 с ключом.
    async function guard(path, body) {
        const invoke = CFG.ipc && window.__TAURI__?.core?.invoke;
        if (CFG.ipc) {
            if (!invoke) throw new Error('нет связи с программой');
            try { return await invoke('guard', { path, body: body ?? null }); } catch (e) { throw new Error(String(e)); }
        }
        const res = await fetch(CFG.url + path, path === '/health' ? {} : { method: 'POST', body: JSON.stringify(body ?? {}),
            headers: { 'Content-Type': 'application/json', 'X-Guard-Token': CFG.token } });
        if (!res.ok) throw new Error('прослойка ответила ' + res.status);
        return res.json();
    }

    // ── Включено ли и в каком состоянии — сайт рисует по этому значок и раздел настроек ──
    const KEY = 'fizmat-guard-enabled';
    const state = { enabled: (() => { try { return localStorage.getItem(KEY) !== '0'; } catch { return true; } })(),
        ready: false, error: '', load_s: null, phase: 'start', progress: 0, total_mb: 0 };
    const announce = () => window.dispatchEvent(new CustomEvent('fizmat-guard', { detail: { ...state } }));
    window.FizmatGuard = {
        get state() { return { ...state }; },
        setEnabled(on) {
            state.enabled = !!on;
            try { localStorage.setItem(KEY, on ? '1' : '0'); } catch {}
            announce();
        },
        vault: () => guard('/vault'),
        forget: () => guard('/vault/clear', {}),
    };

    // ── Окно «что скроем» ───────────────────────────────────────────
    const css = `
.fzg-back{position:fixed;inset:0;background:rgba(0,0,0,.45);z-index:2147483646;display:flex;align-items:center;justify-content:center;font:14px/1.45 Manrope,system-ui,sans-serif}
.fzg{background:var(--bg,#fff);color:var(--text,#151515);width:min(460px,calc(100vw - 32px));border-radius:16px;padding:20px;box-shadow:0 20px 60px rgba(0,0,0,.3)}
.fzg h3{margin:0 0 4px;font-size:17px}.fzg p{margin:0 0 12px;opacity:.7}
.fzg ul{list-style:none;margin:0 0 12px;padding:0;max-height:40vh;overflow:auto}
.fzg li{display:flex;gap:10px;align-items:center;padding:7px 0;border-top:1px solid rgba(127,127,127,.18)}
.fzg li b{font-weight:600;word-break:break-all}.fzg li span{margin-left:auto;font-size:12px;opacity:.6;white-space:nowrap}
.fzg .warn{background:rgba(234,179,8,.15);border-radius:10px;padding:8px 10px;margin-bottom:12px}
.fzg .row{display:flex;gap:8px;justify-content:flex-end;flex-wrap:wrap}
.fzg button{border:1px solid rgba(127,127,127,.35);background:none;color:inherit;border-radius:999px;padding:8px 14px;font:inherit;cursor:pointer}
.fzg button.go{background:#151515;color:#fff;border-color:#151515}
`;
    const esc = (s) => s.replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));

    function ask(found) {
        return new Promise((done) => {
            const back = document.createElement('div');
            back.className = 'fzg-back';
            const items = found.spans.map((s, i) => `<li><input type="checkbox" checked data-i="${i}"><b>${esc(s.value)}</b>`
                + `<span>${esc(KIND[s.kind] || s.kind)} → ${esc(s.tag)}</span></li>`).join('');
            back.innerHTML = `<div class="fzg" role="dialog" aria-modal="true">
<h3>Личные данные в сообщении</h3>
<p>Отмеченное уйдёт в Fizmat метками, настоящие значения останутся на этом компьютере.</p>
${found.topics.length ? `<div class="warn">Похоже на чувствительную тему: ${esc(found.topics.join(', '))}.</div>` : ''}
${items ? `<ul>${items}</ul>` : ''}
<div class="row"><button data-a="cancel">Отменить</button><button data-a="raw">Отправить как есть</button>
<button class="go" data-a="mask">${items ? 'Скрыть и отправить' : 'Отправить'}</button></div></div>`;
            back.addEventListener('click', (e) => {
                const a = e.target.closest('button')?.dataset.a;
                if (!a) return;
                const keep = [...back.querySelectorAll('input:not(:checked)')].map((c) => found.spans[+c.dataset.i].value);
                back.remove();
                done({ action: a, keep });
            });
            document.body.appendChild(back);
            back.querySelector('.go').focus();
        });
    }

    /** Текст вопроса → что отправить на самом деле, или null — отмена. */
    async function gate(text) {
        if (!text || !text.trim() || !state.enabled) return text;
        // Чат Code (папка на компьютере): прослойка только для облачных чатов.
        if (document.querySelector('[data-no-guard]')) return text;
        if (state.phase === 'download' || state.phase === 'loading') {
            const why = state.phase === 'download'
                ? `модель ещё скачивается (${Math.round(state.progress * 100)}%)` : 'модель ещё загружается';
            return confirm(`Защита личных данных не готова: ${why}. Отправить без проверки?`) ? text : null;
        }
        let found;
        try {
            found = await guard('/scan', { text });
        } catch (e) {
            // Прослойка не отвечает — не отправляем молча: человек сам решает.
            return confirm('Защита личных данных не отвечает (' + e.message + '). Отправить без проверки?') ? text : null;
        }
        if (!found.spans.length && !found.topics.length) return text;
        const choice = await ask(found);
        if (choice.action === 'cancel') return null;
        if (choice.action === 'raw') return text;
        return (await guard('/mask', { text, keep: choice.keep })).masked;
    }

    function putBack(text) {
        const field = document.getElementById('input') || document.querySelector('textarea');
        if (field && !field.value) { field.value = text; field.dispatchEvent(new Event('input', { bubbles: true })); }
    }

    // ── Отправка из чата: fetch POST /c/{id}/messages и /queue ─────
    const nativeFetch = window.fetch.bind(window);
    window.fetch = async function (input, init = {}) {
        try {
            const url = new URL(typeof input === 'string' ? input : input.url, location.href);
            const method = (init.method || (typeof input !== 'string' && input.method) || 'GET').toUpperCase();
            if (method === 'POST' && url.origin === location.origin && SEND.test(url.pathname) && typeof init.body === 'string') {
                const body = JSON.parse(init.body);
                if (typeof body.q === 'string' && body.q.trim()) {
                    const sent = await gate(body.q);
                    if (sent === null) {
                        putBack(body.q);
                        return new Response(JSON.stringify({ message: 'Отправка отменена.' }),
                            { status: 422, headers: { 'Content-Type': 'application/json' } });
                    }
                    init = { ...init, body: JSON.stringify({ ...body, q: sent }) };
                }
            }
        } catch (e) { console.warn('FizmatGuard', e); }
        return nativeFetch(input, init);
    };

    // ── Первый вопрос — обычной формой (Главная, новый чат) ─────────
    const nativeSubmit = HTMLFormElement.prototype.submit;
    HTMLFormElement.prototype.submit = function () {
        const q = this.querySelector('[name=q]');
        if (!q || !q.value.trim() || this.dataset.fzgOk) return nativeSubmit.call(this);
        gate(q.value).then((sent) => {
            if (sent === null) { putBack(q.value); return; }
            q.value = sent;
            this.dataset.fzgOk = '1';
            nativeSubmit.call(this);
        });
    };

    // ── Метки в ленте → настоящие значения ──────────────────────────
    const pending = new Set();
    let timer = null;
    function collect(root) {
        const walk = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
        for (let n = walk.currentNode; n; n = walk.nextNode()) {
            if (n.nodeType === 3 && TAG.test(n.nodeValue) && !n.parentElement?.closest('textarea,input,.fzg,[data-guard-raw]')) pending.add(n);
        }
        clearTimeout(timer);
        timer = setTimeout(flush, 30);
    }
    async function flush() {
        const nodes = [...pending];
        pending.clear();
        if (!nodes.length) return;
        try {
            const { texts } = await guard('/restore', { texts: nodes.map((n) => n.nodeValue) });
            nodes.forEach((n, i) => { if (texts[i] !== n.nodeValue) n.nodeValue = texts[i]; });
        } catch (e) { console.warn('FizmatGuard restore', e); }
    }

    function start() {
        const style = document.createElement('style');
        style.textContent = css;
        document.head.appendChild(style);
        // Первый запуск — модель скачивается минуты, потом грузится секунды: спрашиваем, пока не готова.
        // Не отвечает сама служба — 20 попыток, дальше «не отвечает».
        const check = (tries) => guard('/health').then((h) => {
            Object.assign(state, { ready: !!h.ok, phase: h.phase || (h.ok ? 'ready' : 'loading'), error: h.error || '',
                progress: h.progress || 0, total_mb: h.total_mb || 0, load_s: h.load_s ?? null });
            announce();
            if (!h.ok && h.phase !== 'error') setTimeout(() => check(20), 1500);
        }).catch((e) => {
            Object.assign(state, { ready: false, phase: tries > 0 ? 'start' : 'down', error: tries > 0 ? 'запускается' : e.message });
            announce();
            if (tries > 0) setTimeout(() => check(tries - 1), 1500);
        });
        check(20);
        collect(document.body);
        new MutationObserver((list) => {
            for (const m of list) {
                if (m.type === 'characterData') collect(m.target.parentNode || document.body);
                m.addedNodes.forEach((n) => (n.nodeType === 1 || n.nodeType === 3) && collect(n.nodeType === 3 ? n.parentNode : n));
            }
        }).observe(document.body, { childList: true, subtree: true, characterData: true });
    }
    document.readyState === 'loading' ? document.addEventListener('DOMContentLoaded', start) : start();
})();
