// Turns the session called `main` on a throwaway ttm into a Claude session with a canned conversation,
// by intercepting the two API calls the chat view makes. For screenshots only.

const NOW = Math.floor(Date.now() / 1000) - 600;
let seq = 0;
const item = (body) => ({ id: `i${++seq}`, seq, ts: NOW + seq * 20, ...body });

export const items = [
  item({ kind: 'user', text: 'Add a Cloudflare tunnel option to the service and reinstall it. Keep the password at 12 characters.', images: 0 }),
  item({ kind: 'thinking', text: 'The user wants a new tunnel provider. I should look at how localhost.run is wired in src/tunnel.rs first, then add config fields and tests.' }),
  item({
    kind: 'assistant',
    text: [
      "I'll start by reading how the existing tunnel provider works.",
      '',
      '## Plan',
      '',
      '1. Add a `cloudflare` provider that runs `cloudflared`',
      '2. Pass the token through the **environment**, never the command line',
      '3. Document the config in the [README](https://example.com/readme)',
      '',
      '```rust',
      'let mut cmd = Command::new("cloudflared");',
      'cmd.env("TUNNEL_TOKEN", token);',
      '```',
      '',
      '| Field | Meaning |',
      '| --- | --- |',
      '| `tunnel_token` | The Cloudflare tunnel token |',
      '| `tunnel_domain` | The public hostname |',
      '',
      '> Status may show the tunnel as up even when the hostname is not routed.',
    ].join('\n'),
  }),
  item({ kind: 'tool', tool: 'Read', summary: 'src/tunnel.rs', input: 'src/tunnel.rs', output: 'pub enum Provider {\n    LocalhostRun,\n}', status: 'ok', diff: null }),
  item({
    kind: 'tool', tool: 'Edit', summary: 'src/tunnel.rs', input: null, output: 'The file has been updated.', status: 'ok',
    diff: { path: 'src/tunnel.rs', edits: [{ old: 'pub enum Provider {\n    LocalhostRun,\n}', new: 'pub enum Provider {\n    LocalhostRun,\n    Cloudflare,\n}' }] },
  }),
  item({ kind: 'tool', tool: 'Bash', summary: 'cargo test --release', input: 'cargo test --release', output: 'error[E0599]: no variant named `Cloudflare`\n  --> src/tunnel.rs:42:9', status: 'error', diff: null }),
  item({ kind: 'command', text: '/model' }),
  item({ kind: 'notice', text: 'Context compacted' }),
  item({
    kind: 'user', text: 'Here is the log from the last run', images: 0,
    pastes: [{ id: null, text: 'INFO ttm::tunnel: starting\nINFO ttm::tunnel: connected\nWARN ttm::tunnel: hostname not routed' }],
  }),
  item({ kind: 'tool', tool: 'Bash', summary: 'cargo clippy', input: 'cargo clippy', output: null, status: 'running', diff: null }),
  item({
    kind: 'question', state: 'pending', answers: [],
    questions: [{ header: 'Password', question: 'The password is below the 12-character minimum. How should I proceed?', multi_select: false,
      options: [{ label: 'Add --allow-short', description: 'Keep the default minimum, add an explicit override flag' }, { label: 'Use a longer password', description: 'Ask for a new one' }] }],
  }),
];

export const chatResponse = {
  epoch: 1, seq, reset: true, items,
  model: 'claude-opus-5-5', effort: 'high', model_label: null, eligible: true, agent: 'claude', ready: true,
  screen: { activity: 'waiting', prompt: null, tail: [] },
  mode: 'default',
  modes: [{ id: 'default', label: 'Default', hint: 'Ask before edits' }, { id: 'plan', label: 'Plan', hint: 'Plan first' }, { id: 'accept_edits', label: 'Accept edits', hint: 'Edit without asking' }],
  yolo: false, effort_live: 'high', suggestion: null,
  todos: [{ content: 'Add the provider', status: 'completed', active: null }, { content: 'Write tests', status: 'in_progress', active: 'Writing tests' }, { content: 'Update the README', status: 'pending', active: null }],
  plan: null, agents: [],
};

export async function installChatFixture(page) {
  await page.route(/\/api\/sessions$/, async (route) => {
    const res = await route.fetch();
    const list = await res.json();
    for (const s of list) if (s.id === 'main') Object.assign(s, { agent: 'claude', preset: 'claude', has_agent_session: true, activity: 'waiting' });
    await route.fulfill({ response: res, json: list });
  });
  await page.route(/\/api\/sessions\/main\/chat/, (route) => route.fulfill({ json: chatResponse }));
}
