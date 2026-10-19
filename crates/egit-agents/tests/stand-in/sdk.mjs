// A stand-in for Anthropic's Claude Agent SDK, for egit's tests: the same exports the bridge
// uses, answering prompts by their words, with no account and no network.
//
//   hello  streams "Hello"            run   asks to run a command (Bash), then runs it or not
//   ask    asks a question            wait  waits until interrupted
//   mode   says the mode and model    who   says which credentials it was given
//   rich   thinks, writes Markdown, and uses tools (read, search, plan, edit, run, fail, a subagent)
//   edit   asks to edit a file        plan  proposes a plan (ExitPlanMode)
//   images says how many images came with it
//   else   says back what it was told

/** One of the SDK's messages streamed in parts. */
const stream = (event) => ({ type: 'stream_event', event, parent_tool_use_id: null });
/** What Claude says, whole. */
const assistant = (content) => ({
  type: 'assistant',
  parent_tool_use_id: null,
  message: { role: 'assistant', content },
});
/** A tool's result (of a subagent's call, under `parent`). */
const toolResult = (id, content, isError, parent = null) => ({
  type: 'user',
  parent_tool_use_id: parent,
  message: { role: 'user', content: [{ type: 'tool_result', tool_use_id: id, content, is_error: isError }] },
});
/** What a subagent says, under its call. */
const subagent = (parent, content) => ({
  type: 'assistant',
  parent_tool_use_id: parent,
  message: { role: 'assistant', content },
});

/** A reply in Markdown, as Claude writes them. */
const MARKDOWN = [
  '## What I found',
  '',
  'The parser stops **one byte early**: `end` is the length, not the last index.',
  '',
  '- `src/parser.rs` reads the input',
  '- `src/main.rs` calls it twice',
  '',
  '```rust',
  'fn parse(input: &str) -> usize {',
  '    let end = input.len() - 1;',
  '    end',
  '}',
  '```',
  '',
  '| File | Calls |',
  '|---|---|',
  '| main.rs | 2 |',
  '| cli.rs | 1 |',
].join('\n');

/** A long run's output. */
const TESTS = Array.from({ length: 240 }, (_, n) => `test parser::case_${n} ... ok`).join('\n');

export function query({ prompt, options }) {
  let interrupted = null;
  let mode = options.permissionMode;
  let model = options.model ?? 'stand-in';
  const session = options.resume ?? 'stand-in-session';
  /** The images that came with the prompt answered, by their media types. */
  let images = [];
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
    } else if (text === 'rich') {
      yield* rich(options.cwd);
    } else if (text === 'edit') {
      const input = {
        file_path: `${options.cwd}/src/parser.rs`,
        old_string: 'fn parse(input: &str) -> usize {\n    let end = input.len();\n    end\n}',
        new_string: 'fn parse(input: &str) -> usize {\n    let end = input.len() - 1;\n    check(end);\n    end\n}',
      };
      yield assistant([{ type: 'tool_use', id: 'tool-edit', name: 'Edit', input }]);
      const answered = await ask('Edit', input, 'tool-edit');
      const allowed = answered.behavior === 'allow';
      yield toolResult('tool-edit', allowed ? 'The file has been updated.' : answered.message, !allowed);
    } else if (text === 'plan') {
      const input = { plan: '## The plan\n\n1. Read the parser.\n2. Fix the off-by-one in `parse`.\n3. Run `cargo test`.' };
      yield assistant([{ type: 'tool_use', id: 'tool-plan', name: 'ExitPlanMode', input }]);
      const answered = await ask('ExitPlanMode', input, 'tool-plan');
      const allowed = answered.behavior === 'allow';
      yield toolResult('tool-plan', allowed ? 'The plan was approved.' : answered.message, !allowed);
    } else if (text === 'images') {
      yield assistant([{ type: 'text', text: `Got ${images.length} images: ${images.join(', ')}` }]);
    } else if (text === 'who') {
      // As Claude Code would see them: the bridge's environment, or the one it was given.
      const env = options.env ?? process.env;
      const key = env.ANTHROPIC_API_KEY ?? 'none';
      const folder = env.CLAUDE_CONFIG_DIR ?? 'none';
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
      const blocks = typeof content === 'string' ? [{ type: 'text', text: content }] : content;
      images = blocks.filter((block) => block.type === 'image').map((block) => block.source.media_type);
      const said = blocks.filter((block) => block.type === 'text').map((block) => block.text).join('\n');
      yield* answer(said);
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
      return [
        { name: 'review', description: 'Review the changes', argumentHint: '' },
        { name: 'compact', description: 'Compact the conversation', argumentHint: '<instructions>' },
        { name: 'init', description: 'Write a CLAUDE.md for this repository', argumentHint: '' },
      ];
    },
    close() {
      interrupted?.();
    },
  });
}

/** Thinking, Markdown, and every kind of tool line: what a long turn shows. */
async function* rich(cwd) {
  const at = (file) => `${cwd}/${file}`;
  yield stream({ type: 'content_block_start', index: 0, content_block: { type: 'thinking', thinking: '' } });
  yield stream({ type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: 'The parser first.' } });
  yield assistant([{ type: 'thinking', thinking: 'The parser first: where does `end` come from?' }]);
  yield assistant([{ type: 'tool_use', id: 't-read', name: 'Read', input: { file_path: at('src/parser.rs') } }]);
  yield toolResult('t-read', '1\tfn parse(input: &str) -> usize {\n2\t    let end = input.len();\n3\t    end\n4\t}', false);
  yield assistant([{ type: 'tool_use', id: 't-grep', name: 'Grep', input: { pattern: 'parse\\(', path: cwd } }]);
  yield toolResult('t-grep', 'src/main.rs:3:    parse(text);\nsrc/cli.rs:9:    parse(&args[1]);', false);
  yield assistant([{ type: 'text', text: MARKDOWN }]);
  const todos = [
    { content: 'Read the parser', status: 'completed', activeForm: 'Reading the parser' },
    { content: 'Fix the off-by-one', status: 'in_progress', activeForm: 'Fixing the off-by-one' },
    { content: 'Run the tests', status: 'pending', activeForm: 'Running the tests' },
  ];
  yield assistant([{ type: 'tool_use', id: 't-todo', name: 'TodoWrite', input: { todos } }]);
  yield toolResult('t-todo', 'Todos have been modified successfully.', false);
  const edit = {
    file_path: at('src/parser.rs'),
    old_string: 'fn parse(input: &str) -> usize {\n    let end = input.len();\n    end\n}',
    new_string: 'fn parse(input: &str) -> usize {\n    let end = input.len() - 1;\n    check(end);\n    end\n}',
  };
  yield assistant([{ type: 'tool_use', id: 't-edit', name: 'Edit', input: edit }]);
  yield toolResult('t-edit', 'The file has been updated.', false);
  yield assistant([{ type: 'tool_use', id: 't-test', name: 'Bash', input: { command: 'cargo test --workspace', description: 'Run the tests' } }]);
  yield toolResult('t-test', TESTS, false);
  yield assistant([{ type: 'tool_use', id: 't-lint', name: 'Bash', input: { command: 'cargo clippy -- -D warnings' } }]);
  yield toolResult('t-lint', 'error: unused variable: `x`\n  --> src/parser.rs:3:9', true);
  yield { type: 'system', subtype: 'api_retry', attempt: 1, max_retries: 3, retry_delay_ms: 500, error_status: 529 };
  yield assistant([{ type: 'tool_use', id: 't-agent', name: 'Agent', input: { description: 'Look for other callers', prompt: 'Find every caller of parse.', subagent_type: 'Explore' } }]);
  yield subagent('t-agent', [{ type: 'tool_use', id: 't-sub', name: 'Glob', input: { pattern: '**/*.rs' } }]);
  yield toolResult('t-sub', 'src/main.rs\nsrc/cli.rs\nsrc/parser.rs', false, 't-agent');
  yield subagent('t-agent', [{ type: 'text', text: 'Two callers: main.rs and cli.rs.' }]);
  yield toolResult('t-agent', 'Two callers: main.rs and cli.rs.', false);
  yield assistant([{ type: 'text', text: 'Fixed the off-by-one in `parse`; the tests pass.' }]);
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
