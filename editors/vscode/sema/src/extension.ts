import * as vscode from 'vscode';
import * as path from 'path';
import { LanguageClient, LanguageClientOptions, ServerOptions } from 'vscode-languageclient/node';
import { registerNotebookEditor, disposeAllNotebookServers } from './notebook';

let client: LanguageClient | undefined;

const resultDecorationType = vscode.window.createTextEditorDecorationType({
    after: { textDecoration: 'none', fontStyle: 'italic' },
    rangeBehavior: vscode.DecorationRangeBehavior.ClosedOpen,
});

// Map from normalized URI string → decorations
const decorationMap = new Map<string, vscode.DecorationOptions[]>();

function normalizeUri(uri: string): string {
    return vscode.Uri.parse(uri).toString();
}

function reapplyDecorations() {
    for (const editor of vscode.window.visibleTextEditors) {
        const key = editor.document.uri.toString();
        const decorations = decorationMap.get(key);
        if (decorations) {
            editor.setDecorations(resultDecorationType, decorations);
        }
    }
}

export async function activate(context: vscode.ExtensionContext) {
    const outputChannel = vscode.window.createOutputChannel('Sema');

    const semaPath = vscode.workspace.getConfiguration('sema').get<string>('path', 'sema');

    const serverOptions: ServerOptions = {
        command: semaPath,
        args: ['lsp'],
    };

    const clientOptions: LanguageClientOptions = {
        documentSelector: [{ scheme: 'file', language: 'sema' }],
        outputChannel,
        initializationOptions: {
            semaPath: semaPath,
        },
    };

    client = new LanguageClient('sema', 'Sema Language Server', serverOptions, clientOptions);

    // Start the client and wait for it to be ready
    await client.start();

    // Listen for eval results
    client.onNotification('sema/evalResult', (result: any) => {
        const uri = normalizeUri(result.uri);
        const editor = vscode.window.visibleTextEditors.find(
            e => e.document.uri.toString() === uri
        );
        if (!editor) return;

        const range = new vscode.Range(
            result.range.start.line, result.range.start.character,
            result.range.end.line, result.range.end.character,
        );

        const displayText = result.ok
            ? ` => ${(result.value ?? 'nil').substring(0, 120)}`
            : ` => ❌ ${(result.error ?? 'error').substring(0, 100)}`;

        const color = result.ok ? '#88c070' : '#e06060';

        const decoration: vscode.DecorationOptions = {
            range,
            renderOptions: {
                after: { contentText: displayText, color },
            },
        };

        const existing = decorationMap.get(uri) || [];
        const filtered = existing.filter(d => !d.range.isEqual(range));
        filtered.push(decoration);
        decorationMap.set(uri, filtered);

        editor.setDecorations(resultDecorationType, filtered);

        outputChannel.appendLine(
            `[${path.basename(uri)}:${result.range.start.line + 1}] ${result.ok ? '✓' : '✗'} ${result.value ?? result.error ?? ''} (${result.elapsedMs}ms)`
        );
    });

    // Reapply decorations when switching editor tabs
    context.subscriptions.push(
        vscode.window.onDidChangeVisibleTextEditors(() => reapplyDecorations())
    );

    // Command: clear all result decorations
    context.subscriptions.push(
        vscode.commands.registerCommand('sema.clearResults', () => {
            decorationMap.clear();
            for (const editor of vscode.window.visibleTextEditors) {
                editor.setDecorations(resultDecorationType, []);
            }
        })
    );

    // Debugging: the `sema dap` subcommand is a stdio DAP server. Register a
    // factory that launches it; it shares the `sema.path` binary with the LSP.
    context.subscriptions.push(
        vscode.debug.registerDebugAdapterDescriptorFactory('sema', {
            createDebugAdapterDescriptor: () => new vscode.DebugAdapterExecutable(semaPath, ['dap']),
        })
    );

    // Notebooks: opening a .sema-nb file shows the notebook web UI (served by
    // `sema notebook serve`) embedded in the editor pane.
    registerNotebookEditor(context, outputChannel);
}

export function deactivate(): Thenable<void> | undefined {
    disposeAllNotebookServers();
    return client?.stop();
}
