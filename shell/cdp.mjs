// Проверка окна оболочки по протоколу отладки WebView2:
// node cdp.mjs "выражение"   (окно запущено с --remote-debugging-port=9333)
const [target] = (await (await fetch('http://127.0.0.1:9333/json')).json()).filter((t) => t.type === 'page');
const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener('open', r));
ws.send(JSON.stringify({ id: 1, method: 'Runtime.evaluate',
    params: { expression: process.argv[2], awaitPromise: true, returnByValue: true } }));
ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data);
    if (m.id !== 1) return;
    console.log(target.url, '→', JSON.stringify(m.result?.result?.value ?? m.result?.exceptionDetails?.exception?.description ?? m, null, 1));
    process.exit(0);
});
