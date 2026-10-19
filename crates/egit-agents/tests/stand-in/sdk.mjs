// A stand-in for Anthropic's Claude Agent SDK, for egit's tests: the same exports the bridge
// uses, answering prompts by their words, with no account and no network.
//
//   hello  streams "Hello"            run   asks to run a command (Bash), then runs it or not
//   ask    asks a question            wait  waits until interrupted
//   mode   says the mode and model    who   says which credentials it was given
//   else   says back what it was told

/** One of the SDK's messages streamed in parts. */
const stream = (event) => ({ type: 'stream_event', event, parent_tool_use_id: null });
/** What Claude says, whole. */
const assistant = (content) => ({
  type: 'assistant',
  parent_tool_use_id: null,
  message: { role: 'assistant', content },
});
/** A tool's result. */
const toolResult = (id, content, isError) => ({
  type: 'user',
  parent_tool_use_id: null,
  message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content, is_error: isError }] },
});

export function query({ prompt, options }) {
  let interrupted = null;
  let mode = options.permissionMode;
  let model = options.model ?? 'stand-in';
  const session = options.resume ?? 'stand-in-session';
  const result = (extra) => ({
    type: 'result',
    session_id: session,
    duration_ms: 10,
    num_turns: 1,
    total_cost_usd: 0.5,
    modelUsage: {
      'stand-in': { inputTokens: 3, outputTokens: 4, cacheReadInputTokens: 0, cacheCreationInputTokens: 0 },
    },
    ...extra,
  });
  const ask = (tool, input, id) =>
    options.canUseTool(tool, input, {
      signal: new AbortController().signal,
      suggestions: [{ type: 'addRules', rules: [{ toolName: tool }], behavior: 'allow', destination: 'session' }],
      toolUseID: id,
      requestId: `request-${id}`,
    });

  async function* answer(text) {
    if (text === 'hello') {
      yield stream({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } });
      yield stream({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'Hel' } });
      yield stream({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'lo' } });
      yield assistant([{ type: 'text', text: 'Hello' }]);
    } else if (text === 'run') {
      const input = { command: 'echo hi' };
      yield assistant([{ type: 'tool_use', id: 'tool-run', name: 'Bash', input }]);
      const answered = await ask('Bash', input, 'tool-run');
      if (answered.behavior === 'allow') {
        yield toolResult('tool-run', answered.updatedPermissions ? 'hi (always)' : 'hi', false);
      } else {
        yield toolResult('tool-run', answered.message, true);
      }
    } else if (text === 'ask') {
      const input = {
        questions: [
          {
            question: 'Which one?',
            header: 'Pick',
            multiSelect: false,
            options: [
              { label: 'One', description: 'The first' },
              { label: 'Two', description: 'The second' },
            ],
          },
        ],
      };
      yield assistant([{ type: 'tool_use', id: 'tool-ask', name: 'AskUserQuestion', input }]);
      const answered = await ask('AskUserQuestion', input, 'tool-ask');
      yield toolResult('tool-ask', JSON.stringify(answered.updatedInput?.answers ?? null), answered.behavior !== 'allow');
    } else if (text === 'wait') {
      await new Promise((resolve) => {
        interrupted = resolve;
      });
      yield result({ subtype: 'error_during_execution', is_error: true, errors: ['interrupted'] });
      return;
    } else if (text === 'mode') {
      yield assistant([{ type: 'text', text: `Mode ${mode}, model ${model}` }]);
    } else if (text === 'who') {
      const key = process.env.ANTHROPIC_API_KEY ?? 'none';
      const folder = process.env.CLAUDE_CONFIG_DIR ?? 'none';
      yield assistant([{ type: 'text', text: `Key ${key}, folder ${folder}` }]);
    } else {
      yield assistant([{ type: 'text', text: `You said: ${text}` }]);
    }
    yield result({ subtype: 'success', is_error: false, result: 'done' });
  }

  async function* run() {
    yield {
      type: 'system',
      subtype: 'init',
      session_id: session,
      model,
      permissionMode: mode,
      cwd: options.cwd,
      tools: ['Bash', 'Read'],
      slash_commands: [],
    };
    for await (const message of prompt) {
      const content = message.message.content;
      yield* answer(typeof content === 'string' ? content : '');
    }
  }

  const generator = run();
  return Object.assign(generator, {
    async interrupt() {
      interrupted?.();
    },
    async setPermissionMode(next) {
      mode = next;
    },
    async setModel(next) {
      model = next ?? 'stand-in';
    },
    async accountInfo() {
      return { email: 'tester@example.invalid', subscriptionType: 'stand-in' };
    },
    async supportedModels() {
      return [{ value: 'stand-in', displayName: 'Stand-in', description: 'Answers by rote' }];
    },
    async supportedCommands() {
      return [{ name: 'review', description: 'Review the changes', argumentHint: '' }];
    },
    close() {
      interrupted?.();
    },
  });
}

export async function listSessions({ dir }) {
  return [
    { sessionId: 'older', summary: 'An older conversation', lastModified: 1_600_000_000_000, cwd: dir },
    { sessionId: 'newer', summary: 'A newer conversation', lastModified: 1_699_963_200_000, cwd: dir, gitBranch: 'main' },
  ];
}

export async function getSessionMessages(session) {
  return [
    { type: 'user', uuid: 'u1', session_id: session, parent_tool_use_id: null, message: { role: 'user', content: 'Before' } },
    {
      type: 'assistant',
      uuid: 'a1',
      session_id: session,
      parent_tool_use_id: null,
      message: { role: 'assistant', content: [{ type: 'text', text: `Earlier, in ${session}` }] },
    },
  ];
}
