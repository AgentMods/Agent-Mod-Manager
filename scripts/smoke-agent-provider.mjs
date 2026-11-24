import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

export const modelName = 'amm-native-smoke';
export const installFilename = 'smoke_install.jar';
export const diagnosePrompt = '帮我诊断这个 Minecraft 测试目录';
// History derives the title locally from the first message, stripping the request prefix.
export const conversationTitle = '诊断这个 Minecraft 测试目录';
export const installPrompt = `帮我安装已导入的 ${installFilename}`;
export const trashPrompt = '帮我把 smoke-note.txt 移入回收站';
export const cancelPrompt = '验证中途停止这条测试请求';
export const firstDelta = '正在核对测试日志。';
export const diagnosisReply = '诊断：logs/latest.log 第 6 行记录 1.9.0，磁盘版本为 2.0.0。';
export const installReply = '已安装并记录本地测试 Mod；未验证加载器激活。';
export const trashReply = '文件事务已提交，测试笔记已移入回收站。';
export const cancelDelta = '正在等待测试取消。';
const historySummary = '前序测试已诊断 Fabric 日志，并仅在用户批准后执行文件操作。';

const frame = value => `data: ${JSON.stringify(value)}\n\n`;
const choices = (delta, finish = null) => frame({ choices: [{ index: 0, delta, finish_reason: finish }] });
const end = 'data: [DONE]\n\n';

function tool(name, id, args) {
  const data = JSON.stringify(args);
  const split = Math.floor(data.length / 2);
  return choices({ tool_calls: [{ index: 0, id, type: 'function', function: { name, arguments: data.slice(0, split) } }] }) +
    choices({ tool_calls: [{ index: 0, function: { arguments: data.slice(split) } }] });
}

function reply(response, content, usage) {
  response.end(choices({ content }) + choices({}, 'stop') +
    frame({ choices: [], usage: { total_tokens: usage } }) + end);
}

function toolResult(messages, id) {
  const last = messages.at(-1);
  assert.equal(last?.role, 'tool');
  assert.equal(last.tool_call_id, id);
  const result = JSON.parse(last.content);
  assert.ok(!result.error, `Tool ${id} did not succeed: ${result.error}`);
  return result;
}

export function pathIdentity(path) {
  if (process.platform !== 'win32') return path;
  return path.replaceAll('\\', '/').replace(/^\/\/\?\//, '').replace(/[A-Z]/g, c => c.toLowerCase());
}

export async function startSmokeModel(root, note, artifact) {
  const diagnosticGate = Promise.withResolvers();
  const cancellation = Promise.withResolvers();
  const failures = [];
  let requests = 0;
  let summaries = 0;
  const server = createServer((request, response) => {
    void handle(request, response).catch(error => {
      failures.push(error);
      if (!response.headersSent) response.writeHead(400);
      response.end();
    });
  });

  async function handle(request, response) {
    assert.ok(!Object.hasOwn(request.headers, 'authorization'), 'The fixture must not receive credentials');
    if (request.method === 'GET' && request.url === '/v1/models') {
      // Provide a native context window so the runtime does not need a public catalog fallback.
      response.writeHead(200, { 'content-type': 'application/json' });
      response.end(JSON.stringify({ object: 'list', data: [
        { id: modelName, object: 'model', context_length: 131_072 },
      ] }));
      return;
    }
    assert.equal(`${request.method} ${request.url}`, 'POST /v1/chat/completions', 'Unexpected model fixture request');
    const chunks = [];
    let bytes = 0;
    for await (const chunk of request) {
      bytes += chunk.length;
      assert.ok(bytes <= 2 * 1024 * 1024, 'Oversized fixture request');
      chunks.push(chunk);
    }
    const body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
    assert.equal(body.model, modelName);
    assert.equal(body.stream, true);
    assert.equal(body.stream_options?.include_usage, true);
    const { messages } = body;
    if (!Object.hasOwn(body, 'tools')) {
      assert.ok(messages.length >= 2, 'Expected system instructions and a transcript for compression');
      assert.equal(messages[0].role, 'system');
      assert.match(messages[0].content, /Merge the prior summary and the next transcript segment into a concise task handoff/);
      assert.match(messages[0].content, /Output only the handoff; do not use tools\./);
      const transcript = messages.at(-1);
      assert.equal(transcript.role, 'user');
      const languageEvidence = messages.slice(1, -1);
      assert.ok(languageEvidence.length <= 1, 'Expected at most one language evidence note before the transcript');
      for (const note of languageEvidence) {
        assert.equal(note.role, 'assistant');
        assert.match(note.content, /^\[Language evidence only: [^\]\n]+\]\n/);
      }
      const history = transcript.content.trim().split('\n').map(line => JSON.parse(line));
      assert.ok(history.some(message => message.role === 'user'), 'Expected prior conversation turns for compression');
      summaries++;
      response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });
      reply(response, historySummary, 13);
      return;
    }
    assert.ok(Array.isArray(body.tools), 'Expected a model tool registry');
    assert.ok(body.tools.some(entry => entry.function.name === 'diagnose_game'));
    assert.ok(body.tools.some(entry => entry.function.name === 'install_library_mod'));
    assert.ok(body.tools.some(entry => entry.function.name === 'trash_paths'));
    assert.equal(messages[0].role, 'system');
    const encoded = messages[0].content.split('。当前工作区游戏: ')[1]?.split('。selection 为 null')[0];
    assert.ok(encoded, 'Expected game workspace in the Agent system prompt');
    const { selection, games } = JSON.parse(encoded);
    assert.equal(games.length, 1);
    const [currentGame] = games;
    assert.ok(currentGame.id);
    assert.deepEqual(selection, [currentGame.id], 'Expected an explicitly selected fixture game');
    assert.equal(pathIdentity(currentGame.rootPath), pathIdentity(root));
    assert.equal(currentGame.adapterSlug, 'minecraft-java');
    response.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' });

    switch (++requests) {
      case 1: {
        assert.deepEqual(messages.at(-1), { role: 'user', content: diagnosePrompt });
        response.write(choices({ content: firstDelta }));
        await diagnosticGate.promise;
        response.end(tool('diagnose_game', 'smoke-diagnose', { game_id: currentGame.id }) +
          choices({}, 'tool_calls') + end);
        break;
      }
      case 2: {
        const result = toolResult(messages, 'smoke-diagnose');
        assert.equal(result.gameId, currentGame.id);
        assert.equal(result.fabricDisk.status, 'scanned');
        const jar = result.fabricDisk.jars.find(entry => entry.id === 'smoke_diff');
        assert.equal(jar.version, '2.0.0');
        assert.deepEqual(jar.logRelation, { status: 'version_differs', line: 6, version: '1.9.0' });
        reply(response, diagnosisReply, 19);
        break;
      }
      case 3:
        assert.deepEqual(messages.at(-1), { role: 'user', content: installPrompt });
        response.end(tool('install_library_mod', 'smoke-install', { request: {
          gameId: currentGame.id, artifactId: artifact.id, mode: 'adapter', modType: 'java-mod',
        } }) + choices({}, 'tool_calls') + end);
        break;
      case 4: {
        const result = toolResult(messages, 'smoke-install');
        assert.equal(result.status, 'committed');
        assert.match(result.transactionId, /^txn_/);
        assert.match(result.installId, /^install_/);
        assert.equal(result.fileCount, 1);
        reply(response, installReply, 21);
        break;
      }
      case 5:
        assert.deepEqual(messages.at(-1), { role: 'user', content: trashPrompt });
        response.end(tool('trash_paths', 'smoke-trash', {
          paths: [join(root, note)], game_id: currentGame.id, reason: 'native smoke',
        }) + choices({}, 'tool_calls') + end);
        break;
      case 6: {
        const result = toolResult(messages, 'smoke-trash');
        assert.match(result.id, /^txn_/);
        assert.equal(pathIdentity(result.entries[0].operation.src), pathIdentity(join(root, note)));
        reply(response, trashReply, 23);
        break;
      }
      case 7:
        assert.deepEqual(messages.at(-1), { role: 'user', content: cancelPrompt });
        response.once('close', () => cancellation.resolve());
        response.write(choices({ content: cancelDelta }) + tool('trash_paths', 'smoke-cancel', {
          paths: [join(root, 'options.txt')], game_id: currentGame.id, reason: 'must not run',
        }));
        // Deliberately leave out finish_reason and [DONE]. The client must close the stream.
        break;
      default:
        assert.fail('Unexpected additional model request');
    }
  }

  await new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', resolve);
  });
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`,
    allowDiagnosis: diagnosticGate.resolve,
    calls: () => requests,
    summaryCalls: () => summaries,
    assertHealthy() { if (failures.length) throw failures[0]; },
    async waitForCancellation() {
      const controller = new AbortController();
      try {
        await Promise.race([cancellation.promise, delay(5_000, null, { signal: controller.signal })
          .then(() => assert.fail('The client did not close the cancelled model stream'))]);
      } finally {
        controller.abort();
      }
    },
    async close() {
      diagnosticGate.resolve();
      const closed = new Promise(resolve => server.close(resolve));
      server.closeAllConnections();
      await closed;
    },
  };
}
