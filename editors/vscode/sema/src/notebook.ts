import * as vscode from 'vscode';
import * as path from 'path';
import * as net from 'net';
import * as http from 'http';
import { ChildProcess, spawn } from 'child_process';

// One `sema notebook serve` process per opened notebook file, embedded in a
// webview iframe. Mirrors the IntelliJ plugin: the Rust server owns all cell
// execution and state (plain REST, loopback-only), so the editor just hosts its
// web UI. Processes are killed when the tab closes and on extension shutdown.

interface NotebookServer {
    proc: ChildProcess;
    port: number;
}

const servers = new Map<string, NotebookServer>(); // key = file fsPath

function getSemaPath(): string {
    return vscode.workspace.getConfiguration('sema').get<string>('path', 'sema');
}

/** Reserve an ephemeral loopback port so the served URL is deterministic
 *  (the server has its own +100 fallback, which would make the URL unknown). */
function reserveFreePort(): Promise<number> {
    return new Promise((resolve, reject) => {
        const srv = net.createServer();
        srv.unref();
        srv.on('error', reject);
        srv.listen(0, '127.0.0.1', () => {
            const addr = srv.address();
            const port = typeof addr === 'object' && addr ? addr.port : 0;
            srv.close(() => (port ? resolve(port) : reject(new Error('no free port'))));
        });
    });
}

/** Poll until the notebook server answers, since it emits no readiness signal
 *  other than a stderr line. */
function waitForReady(port: number, timeoutMs = 15000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    return new Promise((resolve, reject) => {
        const attempt = () => {
            const req = http.get(
                { host: '127.0.0.1', port, path: '/api/notebook', timeout: 1000 },
                (res) => {
                    res.resume();
                    if (res.statusCode && res.statusCode < 500) resolve();
                    else retry();
                }
            );
            req.on('error', retry);
            req.on('timeout', () => { req.destroy(); retry(); });
        };
        const retry = () => {
            if (Date.now() > deadline) reject(new Error('Sema notebook server did not start in time'));
            else setTimeout(attempt, 100);
        };
        attempt();
    });
}

class SemaNotebookProvider implements vscode.CustomReadonlyEditorProvider {
    constructor(private readonly output: vscode.OutputChannel) {}

    openCustomDocument(uri: vscode.Uri): vscode.CustomDocument {
        return { uri, dispose: () => {} };
    }

    async resolveCustomEditor(
        document: vscode.CustomDocument,
        webviewPanel: vscode.WebviewPanel
    ): Promise<void> {
        const fsPath = document.uri.fsPath;
        webviewPanel.webview.options = { enableScripts: true };
        webviewPanel.webview.html = loadingHtml();

        let server: NotebookServer;
        try {
            server = await this.ensureServer(fsPath);
        } catch (err) {
            this.stopServer(fsPath);
            webviewPanel.webview.html = errorHtml(String(err instanceof Error ? err.message : err));
            return;
        }

        webviewPanel.webview.html = iframeHtml(server.port);
        webviewPanel.onDidDispose(() => this.stopServer(fsPath));
    }

    private async ensureServer(fsPath: string): Promise<NotebookServer> {
        const existing = servers.get(fsPath);
        if (existing && existing.proc.exitCode === null) return existing;

        const port = await reserveFreePort();
        const semaPath = getSemaPath();
        const proc = spawn(
            semaPath,
            ['notebook', 'serve', fsPath, '--host', '127.0.0.1', '--port', String(port)],
            { cwd: path.dirname(fsPath) }
        );
        proc.stderr?.on('data', (d) => this.output.append(d.toString()));

        const server: NotebookServer = { proc, port };
        servers.set(fsPath, server);

        await new Promise<void>((resolve, reject) => {
            let settled = false;
            const done = (fn: () => void) => { if (!settled) { settled = true; fn(); } };
            proc.on('error', (e) =>
                done(() =>
                    reject(
                        new Error(
                            `Could not launch '${semaPath} notebook serve'. Is the sema binary on your PATH or set in the "sema.path" setting? (${e.message})`
                        )
                    )
                )
            );
            proc.on('exit', (code) => done(() => reject(new Error(`sema exited (code ${code}) before serving`))));
            waitForReady(port).then(() => done(resolve), (err) => done(() => reject(err)));
        });
        return server;
    }

    private stopServer(fsPath: string): void {
        const s = servers.get(fsPath);
        if (s) {
            s.proc.kill();
            servers.delete(fsPath);
        }
    }
}

export function registerNotebookEditor(
    context: vscode.ExtensionContext,
    output: vscode.OutputChannel
): void {
    context.subscriptions.push(
        vscode.window.registerCustomEditorProvider(
            'sema.notebook',
            new SemaNotebookProvider(output),
            { webviewOptions: { retainContextWhenHidden: true }, supportsMultipleEditorsPerDocument: false }
        )
    );
}

/** Kill every notebook server — called from the extension's deactivate(). */
export function disposeAllNotebookServers(): void {
    for (const { proc } of servers.values()) proc.kill();
    servers.clear();
}

function frame(body: string): string {
    return `<!DOCTYPE html><html><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; frame-src http://127.0.0.1:* http://localhost:*; style-src 'unsafe-inline'; font-src data:;">
<style>
 html,body{margin:0;padding:0;height:100vh;background:#1a1a1a;color:#e9e3d6;
   font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif}
 .center{display:flex;align-items:center;justify-content:center;height:100vh;text-align:center;padding:2rem}
 iframe{border:0;width:100%;height:100vh;display:block}
 .muted{color:#8a8577;font-size:13px;margin-top:.5rem}
</style></head><body>${body}</body></html>`;
}

function loadingHtml(): string {
    return frame(`<div class="center"><div><div>Starting Sema notebook…</div></div></div>`);
}

function errorHtml(message: string): string {
    const safe = message.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' }[c] as string));
    return frame(`<div class="center"><div><div>Could not open the Sema notebook.</div><div class="muted">${safe}</div></div></div>`);
}

function iframeHtml(port: number): string {
    return frame(
        `<iframe src="http://127.0.0.1:${port}" sandbox="allow-scripts allow-forms allow-same-origin allow-popups"></iframe>`
    );
}
