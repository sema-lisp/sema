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

// Every ToolAnnotations member is set explicitly. No tool is consequential:
// all of them act only on the editor, the debugger, and the virtual filesystem
// of this page. No tool is a `debugging` tool: the debugger tools debug the
// user's Sema program, not the page.
function annotations({ readOnly, untrusted = false }) {
  return {
    readOnlyHint: readOnly,
    untrustedContentHint: untrusted,
    consequentialHint: false,
    debugging: false,
  };
}

const READ_ONLY = annotations({ readOnly: true });
const READ_ONLY_UNTRUSTED = annotations({ readOnly: true, untrusted: true });
const MUTATING = annotations({ readOnly: false });

// Output schemas describe the success shape of each tool. Every tool can also
// return `failureSchema`, so each outputSchema is `oneOf: [success, failure]`.
// Success shapes allow extra properties so that a new field does not break a
// harness that validates output.
const failureSchema = {
  type: 'object',
  properties: {
    ok: { const: false },
    error: {
      type: 'object',
      properties: {
        code: { type: 'string' },
        message: { type: 'string' },
      },
      required: ['code', 'message'],
    },
  },
  required: ['ok', 'error'],
};

function output(properties, required = []) {
  return {
    oneOf: [
      {
        type: 'object',
        properties: { ok: { const: true }, ...properties },
        required: ['ok', ...required],
      },
      failureSchema,
    ],
  };
}

const nullableString = { type: ['string', 'null'] };
const nullableInteger = { type: ['integer', 'null'] };

const pageOutputProperties = {
  content: { type: 'string' },
  offset: { type: 'integer' },
  limit: { type: 'integer' },
  total: { type: 'integer' },
  truncated: { type: 'boolean' },
  next_offset: nullableInteger,
};
const pageOutputRequired = ['content', 'offset', 'total', 'truncated', 'next_offset'];

const breakpointsOutput = { type: 'array', items: { type: 'integer' } };

const debugStateOutput = output({
  state: { type: 'string', enum: ['idle', 'running', 'paused'] },
  status: { type: 'string', enum: ['paused', 'finished', 'error', 'stopped'] },
  line: nullableInteger,
  breakpoints: breakpointsOutput,
  locals: { type: 'array' },
  stack: { type: 'array' },
  value: nullableString,
  error: { type: 'string' },
}, ['state', 'breakpoints', 'locals', 'stack']);

const exampleOutput = {
  type: 'object',
  properties: {
    id: { type: 'string' },
    name: { type: 'string' },
    category: nullableString,
  },
  required: ['id', 'name'],
};

const TOOL_DEFINITIONS = [
  {
    name: 'read_editor',
    title: 'Read editor',
    description: 'Read Sema source from the playground editor. Use this before editing or running unfamiliar code.',
    inputSchema: objectSchema(pageProperties),
    outputSchema: output(pageOutputProperties, pageOutputRequired),
    annotations: READ_ONLY_UNTRUSTED,
  },
  {
    name: 'write_editor',
    title: 'Write editor',
    description: 'Replace the entire playground editor with Sema source. This does not run the code.',
    inputSchema: objectSchema({
      code: { type: 'string', description: 'Complete Sema source to place in the editor.' },
    }, ['code']),
    outputSchema: output({ length: { type: 'integer' } }, ['length']),
    annotations: MUTATING,
  },
  {
    name: 'format_editor',
    title: 'Format editor',
    description: 'Format and replace the current Sema source in the editor. This does not run the code.',
    inputSchema: objectSchema(),
    outputSchema: output({ formatted: { type: 'boolean' }, length: { type: 'integer' } }, ['formatted']),
    annotations: MUTATING,
  },
  {
    name: 'run_editor',
    title: 'Run code',
    description: 'Run the current Sema source, using the worker runtime when available, and wait for evaluation to finish. Cancelling the call stops a worker-backed run.',
    inputSchema: objectSchema(),
    outputSchema: output({
      status: { const: 'finished' },
      value: nullableString,
      elapsed_ms: { type: ['number', 'null'] },
    }, ['status']),
    annotations: MUTATING,
  },
  {
    name: 'stop_run',
    title: 'Stop run',
    description: 'Cancel the active worker-backed Sema evaluation when one is running.',
    inputSchema: objectSchema(),
    outputSchema: output({ status: { const: 'stopping' } }, ['status']),
    annotations: MUTATING,
  },
  {
    name: 'read_output',
    title: 'Read output',
    description: 'Read the current playground output, including values, printed lines, errors, and timing.',
    inputSchema: objectSchema(pageProperties),
    outputSchema: output(pageOutputProperties, pageOutputRequired),
    annotations: READ_ONLY_UNTRUSTED,
  },
  {
    name: 'find_examples',
    title: 'Find examples',
    description: 'Find bundled Sema examples by filename, category, or identifier. Returns identifiers accepted by load_example.',
    inputSchema: objectSchema({
      query: { type: 'string', default: '', description: 'Case-insensitive text used to filter examples.' },
      limit: { type: 'integer', minimum: 1, maximum: 20, default: 20, description: 'Maximum examples to return.' },
    }),
    outputSchema: output({
      examples: { type: 'array', items: exampleOutput },
      total: { type: 'integer' },
      truncated: { type: 'boolean' },
    }, ['examples', 'total', 'truncated']),
    annotations: READ_ONLY,
  },
  {
    name: 'load_example',
    title: 'Load example',
    description: 'Replace the editor with one bundled Sema example selected by identifier or filename. This does not run the code.',
    inputSchema: objectSchema({
      id: { type: 'string', description: 'Example identifier such as getting-started/hello.sema or hello.sema.' },
    }, ['id']),
    outputSchema: output({
      ...exampleOutput.properties,
      length: { type: 'integer' },
    }, ['id', 'name']),
    annotations: MUTATING,
  },
  {
    name: 'list_files',
    title: 'List files',
    description: 'List the immediate children of one virtual filesystem directory. The listing is non-recursive.',
    inputSchema: objectSchema({
      dir: { type: 'string', default: '/', description: 'Absolute virtual directory path to list.' },
    }),
    outputSchema: output({
      dir: { type: 'string' },
      entries: {
        type: 'array',
        items: {
          type: 'object',
          properties: {
            name: { type: 'string' },
            path: { type: 'string' },
            type: { type: 'string', enum: ['file', 'directory'] },
          },
          required: ['name', 'path', 'type'],
        },
      },
    }, ['dir', 'entries']),
    annotations: READ_ONLY_UNTRUSTED,
  },
  {
    name: 'read_file',
    title: 'Read file',
    description: 'Read a character range from a UTF-8 text file in the playground virtual filesystem.',
    inputSchema: objectSchema({
      path: { type: 'string', description: 'Absolute virtual file path to read.' },
      ...pageProperties,
    }, ['path']),
    outputSchema: output({ path: { type: 'string' }, ...pageOutputProperties }, ['path', ...pageOutputRequired]),
    annotations: READ_ONLY_UNTRUSTED,
  },
  {
    name: 'write_file',
    title: 'Write file',
    description: 'Create or overwrite a UTF-8 virtual file, creating parent directories as needed. Content is limited to 1 MiB.',
    inputSchema: objectSchema({
      path: { type: 'string', description: 'Absolute virtual file path to create or replace.' },
      content: { type: 'string', maxLength: 1_048_576, description: 'UTF-8 text content, limited to 1 MiB.' },
    }, ['path', 'content']),
    outputSchema: output({ path: { type: 'string' }, bytes: { type: 'integer' } }, ['path', 'bytes']),
    annotations: MUTATING,
  },
  {
    name: 'set_breakpoints',
    title: 'Set breakpoints',
    description: 'Replace all debugger breakpoints with one-based lines, snapping requests to executable source lines.',
    inputSchema: objectSchema({
      lines: {
        type: 'array',
        items: { type: 'integer', minimum: 1 },
        uniqueItems: true,
        description: 'One-based editor line numbers on which to pause.',
      },
    }, ['lines']),
    outputSchema: output({ breakpoints: breakpointsOutput }, ['breakpoints']),
    annotations: MUTATING,
  },
  {
    name: 'start_debugging',
    title: 'Start debugging',
    description: 'Start debugging the current Sema source and wait for a pause, finish, or error.',
    inputSchema: objectSchema(),
    outputSchema: debugStateOutput,
    annotations: MUTATING,
  },
  {
    name: 'continue_debugging',
    title: 'Continue debugging',
    description: 'Continue the paused Sema debugger and wait for the next breakpoint, finish, or error.',
    inputSchema: objectSchema(),
    outputSchema: debugStateOutput,
    annotations: MUTATING,
  },
  {
    name: 'step_debugger',
    title: 'Step debugger',
    description: 'Step the paused Sema debugger into, over, or out, then wait for its next stable state.',
    inputSchema: objectSchema({
      mode: { type: 'string', enum: ['into', 'over', 'out'], description: 'Debugger stepping mode.' },
    }, ['mode']),
    outputSchema: debugStateOutput,
    annotations: MUTATING,
  },
  {
    name: 'stop_debugging',
    title: 'Stop debugging',
    description: 'Stop the active Sema debugger and return the playground to its idle state.',
    inputSchema: objectSchema(),
    outputSchema: debugStateOutput,
    annotations: MUTATING,
  },
  {
    name: 'get_debug_state',
    title: 'Get debug state',
    description: 'Read debugger status, active line, breakpoints, locals, and stack frames.',
    inputSchema: objectSchema(),
    outputSchema: debugStateOutput,
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
      // The browser passes `{ signal }` as the second argument. The signal
      // aborts when the user or the agent cancels this call.
      execute: async (input = {}, { signal } = {}) => {
        const action = actions[definition.name];
        if (typeof action !== 'function') {
          return failure('NOT_READY', 'The Sema playground is still initializing.');
        }
        if (signal?.aborted) return failure('CANCELLED', 'The tool call was cancelled.');
        try {
          return await action(input ?? {}, { signal });
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
