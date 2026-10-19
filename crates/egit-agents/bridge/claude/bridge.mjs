// egit's bridge to Claude: Anthropic's Claude Agent SDK, which runs Claude Code itself (its
// system prompt, tools, settings, CLAUDE.md files, hooks, MCP servers, and sessions), driven
// by JSON lines on standard input and answering in JSON lines on standard output.
//
// Run with no arguments it holds one conversation:
//
//   in   {"op":"start","cwd":…,"resume":…,"model":…,"mode":…}  once, first
//        {"op":"send","text":…,"images":[{"mediaType":…,"data":…}]}  a prompt (images base64)
//        {"op":"answer","request":…,"allow":…,"always":…,"input":…,"message":…,"interrupt":…}
//        {"op":"interrupt"} {"op":"mode","mode":…} {"op":"model","model":…} {"op":"info"}
//   out  {"event":"ready"} {"event":"message","message":<an SDK message, as the SDK gives it>}
//        {"event":"info",…} once started, unasked: who is signed in, the models, the commands
//        {"event":"permission","request":…,"tool":…,"input":…,"suggestions":…,…}
//        {"event":"withdrawn","request":…} {"event":"info",…} {"event":"failed","op":…,"message":…}
//        {"event":"ended","error":…}
//
// Its standard input closing ends the conversation, and the bridge. With arguments it answers
// once and exits: `sessions <dir> <limit>` (the conversations kept for a folder) or
// `history <dir> <session>` (one's messages), as one JSON value.
//
// `EGIT_CLAUDE_SDK` names another module in the SDK's place (egit's tests use a stand-in).

import { createInterface } from 'node:readline';
import { pathToFileURL } from 'node:url';

const sdk = await import(
  process.env.EGIT_CLAUDE_SDK
    ? pathToFileURL(process.env.EGIT_CLAUDE_SDK).href
    : '@anthropic-ai/claude-agent-sdk'
);

/** Writes one line of JSON to egit. */
function emit(value) {
  process.stdout.write(`${JSON.stringify(value)}\n`);
}

/** What went wrong, in words. */
function said(error) {
  return error instanceof Error ? error.message : String(error);
}

const [command, ...args] = process.argv.slice(2);
if (command === 'sessions') {
  const [dir, limit] = args;
  emit(await sdk.listSessions({ dir, limit: Number(limit) || undefined }));
} else if (command === 'history') {
  const [dir, session] = args;
  emit(await sdk.getSessionMessages(session, { dir }));
} else if (command !== undefined) {
  emit({ event: 'failed', op: command, message: `unknown command ${command}` });
  process.exitCode = 2;
} else {
  await converse();
}

/** Prompts waiting for the SDK to take them: an async iterable egit's `send`s feed. */
function inbox() {
  const queued = [];
  const waiting = [];
  let closed = false;
  return {
    push(message) {
      const next = waiting.shift();
      if (next) next({ value: message, done: false });
      else queued.push(message);
    },
    close() {
      closed = true;
      for (const next of waiting.splice(0)) next({ value: undefined, done: true });
    },
    [Symbol.asyncIterator]() {
      return {
        next() {
          if (queued.length > 0) return Promise.resolve({ value: queued.shift(), done: false });
          if (closed) return Promise.resolve({ value: undefined, done: true });
          return new Promise((resolve) => waiting.push(resolve));
        },
      };
    },
  };
}

/** One conversation, as egit's lines ask. */
async function converse() {
  const prompts = inbox();
  const abort = new AbortController();
  /** Permission requests waiting for egit's answer, by their id. */
  const pending = new Map();
  let asked = 0;
  /** The end of what Claude Code printed to its standard error, for a failure's words. */
  let stderr = '';
  let query;
  let ended;

  /** Asks egit whether a tool may run; resolves with its answer. */
  function canUseTool(tool, input, options) {
    asked += 1;
    const request = options.requestId ?? `egit-${asked}`;
    return new Promise((resolve) => {
      const withdraw = () => {
        if (pending.delete(request)) {
          emit({ event: 'withdrawn', request });
          resolve({ behavior: 'deny', message: 'The request was withdrawn.' });
        }
      };
      pending.set(request, { resolve, input, suggestions: options.suggestions ?? [], withdraw });
      options.signal?.addEventListener('abort', withdraw, { once: true });
      emit({
        event: 'permission',
        request,
        tool,
        input,
        suggestions: options.suggestions ?? [],
        reason: options.decisionReason ?? null,
        blockedPath: options.blockedPath ?? null,
        toolUseId: options.toolUseID ?? null,
        agentId: options.agentID ?? null,
      });
    });
  }

  /** Carries out one of egit's lines. */
  async function handle(line) {
    const request = JSON.parse(line);
    switch (request.op) {
      case 'start': {
        if (query) throw new Error('the conversation has started already');
        query = sdk.query({
          prompt: prompts,
          options: {
            cwd: request.cwd,
            resume: request.resume ?? undefined,
            model: request.model ?? undefined,
            // Explicitly: left out, Claude Code may start in auto mode.
            permissionMode: request.mode ?? 'default',
            // Claude Code's own system prompt, recorded for the conversation.
            systemPrompt: { type: 'preset', preset: 'claude_code', snapshot: true },
            // The user's, the project's, and the local settings, as the terminal reads them.
            settingSources: ['user', 'project', 'local'],
            includePartialMessages: true,
            canUseTool,
            abortController: abort,
            stderr: (data) => {
              stderr = (stderr + data).slice(-8192);
            },
          },
        });
        ended = pump(query);
        // What the window offers (the models, the slash commands) and who it runs as, as soon
        // as Claude Code has started, before anything is sent.
        info(query).catch((failure) => emit({ event: 'failed', op: 'info', message: said(failure) }));
        break;
      }
      case 'send': {
        const images = (request.images ?? []).map((image) => ({
          type: 'image',
          source: { type: 'base64', media_type: image.mediaType, data: image.data },
        }));
        const content = images.length > 0 ? [...images, { type: 'text', text: request.text }] : request.text;
        prompts.push({
          type: 'user',
          message: { role: 'user', content },
          parent_tool_use_id: null,
        });
        break;
      }
      case 'answer': {
        const waiting = pending.get(request.request);
        if (!waiting) break;
        pending.delete(request.request);
        const input = request.input ?? waiting.input;
        waiting.resolve(
          request.allow
            ? {
                behavior: 'allow',
                updatedInput: input,
                ...(request.always ? { updatedPermissions: waiting.suggestions } : {}),
              }
            : {
                behavior: 'deny',
                message: request.message || 'The user refused this.',
                interrupt: Boolean(request.interrupt),
              },
        );
        break;
      }
      case 'interrupt':
        await started().interrupt();
        break;
      case 'mode':
        await started().setPermissionMode(request.mode);
        break;
      case 'model':
        await started().setModel(request.model ?? undefined);
        break;
      case 'info':
        await info(started());
        break;
      default:
        throw new Error(`unknown op ${request.op}`);
    }
  }

  /** Tells egit who is signed in, and the models and commands there are. */
  async function info(running) {
    const [account, models, commands] = await Promise.all([
      running.accountInfo(),
      running.supportedModels(),
      running.supportedCommands(),
    ]);
    emit({ event: 'info', account, models, commands });
  }

  function started() {
    if (!query) throw new Error('the conversation hasn’t started');
    return query;
  }

  /** Hands every SDK message to egit until the conversation ends. */
  async function pump(running) {
    let error = null;
    try {
      for await (const message of running) emit({ event: 'message', message });
    } catch (failure) {
      error = said(failure);
      if (stderr.trim()) error += `\n${stderr.trim()}`;
    }
    emit({ event: 'ended', error });
  }

  emit({ event: 'ready' });
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  for await (const line of lines) {
    if (!line.trim()) continue;
    let op = null;
    try {
      op = JSON.parse(line).op ?? null;
    } catch {
      // `handle` says what is wrong with it.
    }
    // Not awaited: an answer may be needed while an interrupt or a question to Claude Code is
    // under way. Each op does its synchronous part (a start, a prompt queued) in order.
    handle(line).catch((failure) => emit({ event: 'failed', op, message: said(failure) }));
  }
  // egit closed its end: the conversation ends.
  prompts.close();
  for (const waiting of [...pending.values()]) waiting.withdraw();
  if (query) {
    query.close();
    await ended;
  }
}
