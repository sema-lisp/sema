const MAX_PAGE_CHARS = 12_000;

function objectSchema(properties = {}, required = []) {
  const schema = {
    type: 'object',
    properties,
    additionalProperties: false,
  };
  if (required.length > 0) schema.required = required;
  return schema;
}

const pageProperties = {
  offset: {
    type: 'integer',
    minimum: 0,
    default: 0,
    description: 'Zero-based character offset at which to start reading.',
  },
  limit: {
    type: 'integer',
    minimum: 1,
    maximum: MAX_PAGE_CHARS,
    default: 1500,
    description: 'Maximum number of characters to return.',
  },
};

const READ_ONLY = { readOnlyHint: true };
const READ_ONLY_UNTRUSTED = { readOnlyHint: true, untrustedContentHint: true };
const MUTATING = { readOnlyHint: false };

const TOOL_DEFINITIONS = [
  {
    name: 'read_editor',
    description: 'Read Sema source from the playground editor. Use this before editing or running unfamiliar code.',
    inputSchema: objectSchema(pageProperties),
    annotations: READ_ONLY_UNTRUSTED,
  },
  {
    name: 'write_editor',
    description: 'Replace the entire playground editor with Sema source. This does not run the code.',
    inputSchema: objectSchema({
      code: { type: 'string', description: 'Complete Sema source to place in the editor.' },
    }, ['code']),
    annotations: MUTATING,
  },
  {
    name: 'format_editor',
    description: 'Format and replace the current Sema source in the editor. This does not run the code.',
    inputSchema: objectSchema(),
    annotations: MUTATING,
  },
  {
    name: 'run_editor',
    description: 'Run the current Sema source, using the worker runtime when available, and wait for evaluation to finish.',
    inputSchema: objectSchema(),
    annotations: MUTATING,
  },
  {
    name: 'stop_run',
    description: 'Cancel the active worker-backed Sema evaluation when one is running.',
    inputSchema: objectSchema(),
    annotations: MUTATING,
  },
  {
    name: 'read_output',
    description: 'Read the current playground output, including values, printed lines, errors, and timing.',
    inputSchema: objectSchema(pageProperties),
    annotations: READ_ONLY_UNTRUSTED,
  },
  {
    name: 'find_examples',
    description: 'Find bundled Sema examples by filename, category, or identifier. Returns identifiers accepted by load_example.',
    inputSchema: objectSchema({
      query: { type: 'string', default: '', description: 'Case-insensitive text used to filter examples.' },
      limit: { type: 'integer', minimum: 1, maximum: 20, default: 20, description: 'Maximum examples to return.' },
    }),
    annotations: READ_ONLY,
  },
  {
    name: 'load_example',
    description: 'Replace the editor with one bundled Sema example selected by identifier or filename. This does not run the code.',
    inputSchema: objectSchema({
      id: { type: 'string', description: 'Example identifier such as getting-started/hello.sema or hello.sema.' },
    }, ['id']),
    annotations: MUTATING,
  },
  {
    name: 'list_files',
    description: 'List the immediate children of one virtual filesystem directory. The listing is non-recursive.',
    inputSchema: objectSchema({
      dir: { type: 'string', default: '/', description: 'Absolute virtual directory path to list.' },
    }),
    annotations: READ_ONLY_UNTRUSTED,
  },
  {
    name: 'read_file',
    description: 'Read a character range from a UTF-8 text file in the playground virtual filesystem.',
    inputSchema: objectSchema({
      path: { type: 'string', description: 'Absolute virtual file path to read.' },
      ...pageProperties,
    }, ['path']),
    annotations: READ_ONLY_UNTRUSTED,
  },
  {
    name: 'write_file',
    description: 'Create or overwrite a UTF-8 virtual file, creating parent directories as needed. Content is limited to 1 MiB.',
    inputSchema: objectSchema({
      path: { type: 'string', description: 'Absolute virtual file path to create or replace.' },
      content: { type: 'string', maxLength: 1_048_576, description: 'UTF-8 text content, limited to 1 MiB.' },
    }, ['path', 'content']),
    annotations: MUTATING,
  },
  {
    name: 'set_breakpoints',
    description: 'Replace all debugger breakpoints with one-based lines, snapping requests to executable source lines.',
    inputSchema: objectSchema({
      lines: {
        type: 'array',
        items: { type: 'integer', minimum: 1 },
        uniqueItems: true,
        description: 'One-based editor line numbers on which to pause.',
      },
    }, ['lines']),
    annotations: MUTATING,
  },
  {
    name: 'start_debugging',
    description: 'Start debugging the current Sema source and wait for a pause, finish, or error.',
    inputSchema: objectSchema(),
    annotations: MUTATING,
  },
  {
    name: 'continue_debugging',
    description: 'Continue the paused Sema debugger and wait for the next breakpoint, finish, or error.',
    inputSchema: objectSchema(),
    annotations: MUTATING,
  },
  {
    name: 'step_debugger',
    description: 'Step the paused Sema debugger into, over, or out, then wait for its next stable state.',
    inputSchema: objectSchema({
      mode: { type: 'string', enum: ['into', 'over', 'out'], description: 'Debugger stepping mode.' },
    }, ['mode']),
    annotations: MUTATING,
  },
  {
    name: 'stop_debugging',
    description: 'Stop the active Sema debugger and return the playground to its idle state.',
    inputSchema: objectSchema(),
    annotations: MUTATING,
  },
  {
    name: 'get_debug_state',
    description: 'Read debugger status, active line, breakpoints, locals, and stack frames.',
    inputSchema: objectSchema(),
    annotations: READ_ONLY_UNTRUSTED,
  },
];

function failure(code, message) {
  return { ok: false, error: { code, message } };
}

export async function registerPlaygroundWebMcp(actions = {}) {
  const modelContext = document.modelContext;
  if (!modelContext || typeof modelContext.registerTool !== 'function') return null;

  const controller = new AbortController();
  try {
    const registrations = TOOL_DEFINITIONS.map((definition) => modelContext.registerTool({
      ...definition,
      execute: async (input = {}) => {
        const action = actions[definition.name];
        if (typeof action !== 'function') {
          return failure('NOT_READY', 'The Sema playground is still initializing.');
        }
        try {
          return await action(input);
        } catch (error) {
          return failure(error?.code ?? 'INTERNAL_ERROR', error?.message ?? String(error));
        }
      },
    }, { signal: controller.signal }));
    await Promise.all(registrations);
  } catch (error) {
    controller.abort();
    throw error;
  }
  return controller;
}
