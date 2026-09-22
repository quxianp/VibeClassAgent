/* ============================================================================
   VibeClassAgent 界面脚本（原生 JS，无框架、无构建步骤）
   ----------------------------------------------------------------------------
   为什么不用 React/Vue：这是个单机工具界面，交互量很小；引一套框架就得配
   npm + 打包，而发布形态是"拷个文件夹过去双击"。原生 JS 够用，且改一行
   刷新就能看到，调试成本最低。

   文案全部来自语言文件（后端 /api/i18n 提供 `web.*` 段），这里不写死中文 ——
   否则 CLI 有语言文件、界面没有，换语言等于只换一半。
   ========================================================================== */

const TOKEN = new URLSearchParams(location.search).get('t') || '';
const view = document.getElementById('view');
const titleEl = document.getElementById('page-title');

/** 语言表：key -> 文案。 */
let L = {};

/** 取一条文案；缺翻译时返回 key 本身（一眼能看出漏了哪条）。 */
function t(key, vars) {
  let s = L[key];
  if (s === undefined) return key;
  if (vars) {
    for (const k of Object.keys(vars)) s = s.split('{' + k + '}').join(vars[k]);
  }
  return s;
}

/** 当前页面缓存的数据。 */
const state = {
  status: null, llm: null, push: null, providers: [],
  timetable: null, schedule: null,
};

/* ------------------------------------------------------------------ 工具 */

async function api(path, body) {
  const sep = path.includes('?') ? '&' : '?';
  const opts = { method: body === undefined ? 'GET' : 'POST' };
  if (body !== undefined) {
    opts.headers = { 'Content-Type': 'application/json' };
    opts.body = JSON.stringify(body);
  }
  const res = await fetch(`${path}${sep}t=${TOKEN}`, opts);
  return res.json();
}

function toast(title, detail, kind) {
  const wrap = document.getElementById('toasts');
  const el = document.createElement('div');
  el.className = 'toast' + (kind ? ' ' + kind : '');
  el.innerHTML = `<div class="t"></div><div class="d"></div>`;
  el.querySelector('.t').textContent = title;
  if (detail) el.querySelector('.d').textContent = detail;
  wrap.appendChild(el);
  setTimeout(() => el.remove(), kind === 'err' ? 9000 : 4200);
}

function esc(s) {
  return String(s == null ? '' : s).replace(/[&<>"]/g, c =>
    ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
}

function el(html) {
  const tpl = document.createElement('template');
  tpl.innerHTML = html.trim();
  return tpl.content.firstElementChild;
}

/** 把界面上的 data-i18n 占位替换成当前语言。 */
function applyStatic() {
  document.querySelectorAll('[data-i18n]').forEach(n => {
    const v = t(n.dataset.i18n);
    if (v !== n.dataset.i18n) n.textContent = v;
  });
}

/** 渲染左上角的 ASCII 字符画；语言文件里没有就退回纯文字。 */
function paintBrand() {
  // 语言文件里 logo 是一整段多行字符串（JSON 里写数组要额外转义，不划算），
  // 所以这里两种形态都接：数组按行拼，字符串直接按 \n 切。
  const raw = L['logo'];
  const lines = Array.isArray(raw) ? raw : String(raw == null ? '' : raw).split('\n');
  const pre = document.getElementById('brand-logo');
  const txt = document.getElementById('brand-text');
  if (lines.some(s => s.trim().length)) {
    pre.textContent = lines.join('\n');
    pre.style.display = '';
    txt.style.display = 'none';
  } else {
    pre.style.display = 'none';
    txt.style.display = '';
  }
}

/* ------------------------------------------------------------------ 概览 */

async function renderOverview() {
  const s = await api('/api/status');
  state.status = s;
  if (!s.ok) { view.innerHTML = `<div class="empty">${esc(s.error)}</div>`; return; }

  paintDots(s);

  const stats = [
    [t('nav.model'), s.llm.ready ? t('ov.model_ready') : t('ov.model_bad'),
     s.llm.ready ? 'ok' : 'bad', s.llm.model || '—'],
    [t('nav.push'), s.push.ready ? s.push.provider : t('ov.push_ready'),
     s.push.ready ? 'ok' : 'bad', s.push.target || '—'],
    [t('side.stt'), s.whisper_ready ? t('ov.stt_ready') : t('ov.stt_bad'),
     s.whisper_ready ? 'ok' : 'bad', 'whisper.cpp'],
    [t('ov.jobs'), String(s.jobs.total), '', `${t('ov.pending')} ${s.jobs.pending}`],
  ];

  let grid = '<div class="grid">';
  for (const [k, v, cls, sub] of stats) {
    grid += `<div class="stat">
      <div class="k">${esc(k)}</div>
      <div class="v ${cls}">${esc(v)}</div>
      <div class="k" style="margin:6px 0 0">${esc(sub)}</div>
    </div>`;
  }
  grid += '</div>';

  const next = `
    <div class="note">
      <b>1. ${esc(t('ov.start_1'))}</b> —— ${esc(t('ov.start_1_d'))}<br>
      <b>2. ${esc(t('ov.start_2'))}</b> —— ${esc(t('ov.start_2_d'))}<br>
      <b>3. ${esc(t('ov.start_3'))}</b> —— ${esc(t('ov.start_3_d'))}<br>
      <br>${esc(t('ov.start_footer'))}
    </div>`;

  view.innerHTML = grid + `
  <div class="card">
    <h2>${esc(t('ov.locations'))}</h2>
    <p class="hint">${esc(t('ov.locations_hint'))}</p>
    <table>
      <tr><th style="width:120px">${esc(t('ov.config'))}</th><td>${esc(s.config_root)}</td></tr>
      <tr><th>${esc(t('ov.data'))}</th><td>${esc(s.data_root)}</td></tr>
    </table>
  </div>
  <div class="card">
    <h2>${esc(t('ov.start_title'))}</h2>
    <p class="hint">${esc(t('ov.start_hint'))}</p>
    ${next}
  </div>`;
}

function paintDots(s) {
  const pill = document.getElementById('pill-profile');
  if (pill) pill.textContent = s.profile || 'default';
  const set = (id, on) => {
    const d = document.getElementById(id);
    if (d) d.className = 'dot ' + (on ? 'on' : 'off');
  };
  set('dot-llm', s.llm.ready);
  set('dot-push', s.push.ready);
  set('dot-stt', s.whisper_ready);
}

/* ------------------------------------------------------------------ 运行 */

async function renderRun() {
  const d = await api('/api/daemon/status');
  const s = state.status || (await api('/api/status'));
  const dot = d.running ? 'ok' : 'bad';

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('run.title'))}</h2>
    <p class="hint">${t('run.hint')}</p>

    <div class="grid">
      <div class="stat">
        <div class="k">${esc(t('run.state'))}</div>
        <div class="v ${dot}">${esc(d.running ? t('run.running') : t('run.stopped'))}</div>
        <div class="k" style="margin:6px 0 0">${d.running ? esc(d.uptime) : '—'}</div>
      </div>
      <div class="stat">
        <div class="k">${esc(t('run.model_push'))}</div>
        <div class="v small ${s.llm.ready ? 'ok' : 'bad'}">${esc(s.llm.ready ? t('ov.model_ready') : t('ov.model_bad'))}</div>
        <div class="k" style="margin:6px 0 0">${s.push.ready ? esc(s.push.provider) : esc(t('run.push_not_set'))}</div>
      </div>
      <div class="stat">
        <div class="k">${esc(t('run.mode'))}</div>
        <div class="v small ${d.dry_run ? 'bad' : ''}">${d.running ? esc(d.dry_run ? t('run.mode_dry') : t('run.mode_real')) : '—'}</div>
        <div class="k" style="margin:6px 0 0">${d.running ? '' : esc(t('run.mode_na'))}</div>
      </div>
    </div>

    <div class="row wrap">
      <button class="btn primary" id="btn-start" ${d.running ? 'disabled' : ''}>${esc(t('run.start'))}</button>
      <button class="btn" id="btn-dry" ${d.running ? 'disabled' : ''}>${esc(t('run.start_dry'))}</button>
      <button class="btn danger" id="btn-stop" ${d.running ? '' : 'disabled'}>${esc(t('run.stop'))}</button>
      <span class="spacer"></span>
      <button class="btn ghost sm" id="btn-log">${esc(t('run.log'))}</button>
    </div>

    <div class="note" id="run-note" style="display:none"></div>
  </div>`;

  const note = document.getElementById('run-note');
  const show = (html, kind) => {
    note.style.display = 'block';
    note.innerHTML = html;
    note.style.borderLeftColor =
      kind === 'err' ? '#e05c5c' : kind === 'warn' ? '#d9a343' : '#2f2f2f';
  };

  // 启动前把「还缺什么」说清楚：等它跑起来什么都不干，用户只会以为坏了
  const precheck = () => {
    const miss = [];
    if (!s.llm.ready) miss.push(t('run.miss_llm'));
    if (!s.push.ready) miss.push(t('run.miss_push'));
    if (!s.whisper_ready) miss.push(t('run.miss_stt'));
    return miss;
  };

  const start = async (dry) => {
    const miss = precheck();
    const warn = miss.length ? miss.join('\n· ') + '\n\n' : '';
    if (!confirm(warn + t(dry ? 'run.dry_confirm' : 'run.start_confirm'))) return;
    const r = await api('/api/daemon/start', { dry_run: dry });
    if (r.ok) {
      const w = r.warnings || [];
      show(`<b style="color:#7fd3ba">${esc(t(dry ? 'run.started_dry' : 'run.started'))}</b>` +
        (w.length ? `<br>${esc(t('run.also_note'))}<br>· ` + w.map(esc).join('<br>· ') : '') +
        `<br><br>${esc(t('run.auto_refresh'))}`);
      setTimeout(renderRun, 1200);
    } else {
      show(`<b style="color:#e05c5c">${esc(t('run.start_failed'))}</b><br><code>${esc(r.error || '')}</code>`, 'err');
    }
  };

  document.getElementById('btn-start').addEventListener('click', () => start(false));
  document.getElementById('btn-dry').addEventListener('click', () => start(true));
  document.getElementById('btn-stop').addEventListener('click', async () => {
    const r = await api('/api/daemon/stop', {});
    if (r.ok) {
      show(esc(t('run.stop_requested')) + (r.note ? '<br>' + esc(r.note) : '') +
        '<br>' + esc(t('run.stop_wait')));
      setTimeout(renderRun, 2500);
    } else {
      show(esc(r.error || ''), 'err');
    }
  });

  document.getElementById('btn-log').addEventListener('click', async () => {
    const l = await api('/api/logs');
    if (!l.ok) { show(esc(l.error || ''), 'err'); return; }
    const lines = l.lines || [];
    show(`<b>${esc(t('run.log_title'))}</b>（${esc(t('run.log_recent'))} ${lines.length} ` +
      `${esc(t('run.log_lines'))} / ${esc(t('run.log_total'))} ${l.total_lines || 0}）<br>
      <code style="display:block;max-height:280px;overflow:auto;margin-top:8px;white-space:pre-wrap">${
        lines.length ? esc(lines.join('\n')) : esc(t('run.log_empty'))}</code>`);
  });

  if (d.last_error) {
    show(`<b style="color:#e05c5c">${esc(t('run.last_error'))}</b><br><code>${esc(d.last_error)}</code>`, 'err');
  }
  if (d.running) {
    setTimeout(() => { if (current === 'run') renderRun(); }, 5000);
  }
}

/* ------------------------------------------------------------------ 模型 */

async function renderModel() {
  const [cfg, prov] = await Promise.all([api('/api/config/llm'), api('/api/providers')]);
  state.llm = cfg;
  state.providers = prov.providers || [];
  if (!cfg.ok) { view.innerHTML = `<div class="empty">${esc(cfg.error)}</div>`; return; }

  const opts = [`<option value="">${esc(t('model.manual'))}</option>`]
    .concat(state.providers.map(p =>
      `<option value="${esc(p.id)}" data-url="${esc(p.base_url)}" ${p.id === cfg.provider ? 'selected' : ''}>${esc(p.name)}${p.note ? ' · ' + esc(p.note) : ''}</option>`))
    .join('');

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('model.title'))}</h2>
    <p class="hint">${t('model.hint')}</p>

    <label class="field"><span>${esc(t('model.provider'))}</span>
      <select id="prov">${opts}</select>
    </label>

    <label class="field"><span>${esc(t('model.base_url'))}</span>
      <input type="text" id="base" value="${esc(cfg.base_url)}" placeholder="https://api.deepseek.com/v1">
    </label>

    <label class="field"><span>${esc(t('model.model'))}</span>
      <div class="row">
        <input type="text" id="model" value="${esc(cfg.model)}" placeholder="deepseek-chat">
        <button class="btn sm" id="btn-models">${esc(t('model.fetch'))}</button>
      </div>
      <div class="note" id="models-note" style="display:none"></div>
    </label>

    <label class="field"><span>${esc(t('model.api_key'))} ${
      cfg.key_set ? `<span class="tag ok">${esc(t('model.key_set'))}</span>`
                  : `<span class="tag bad">${esc(t('model.key_unset'))}</span>`}</span>
      <input type="password" id="key" placeholder="${esc(t('model.key_ph'))}">
    </label>

    <label class="field"><span>${esc(t('model.peak'))}</span>
      <label style="display:flex;align-items:center;gap:8px;font-size:13px;color:var(--text-dim)">
        <input type="checkbox" id="defer" style="width:auto" ${cfg.defer_on_peak ? 'checked' : ''}>
        ${esc(t('model.peak_label'))}
      </label>
    </label>

    <div class="row">
      <button class="btn primary" id="btn-save">${esc(t('common.save'))}</button>
      <span class="spacer"></span>
    </div>

    <div class="note">${t('model.peak_note')}</div>
  </div>`;

  document.getElementById('prov').addEventListener('change', e => {
    const url = e.target.selectedOptions[0]?.dataset?.url;
    if (url) document.getElementById('base').value = url;
  });

  document.getElementById('btn-save').addEventListener('click', async () => {
    const payload = {
      provider: document.getElementById('prov').value,
      base_url: document.getElementById('base').value.trim(),
      model: document.getElementById('model').value.trim(),
      defer_on_peak: document.getElementById('defer').checked,
    };
    const k = document.getElementById('key').value.trim();
    if (k) payload.api_key = k;
    const r = await api('/api/config/llm', payload);
    if (r.ok) {
      toast(t('common.saved'), t('common.wrote_fields') + (r.wrote || []).join('、'));
      document.getElementById('key').value = '';
      renderModel();
      refreshDots();
    } else {
      toast(t('common.save_failed'), r.error || t('common.unknown'), 'err');
    }
  });

  document.getElementById('btn-models').addEventListener('click', async () => {
    const note = document.getElementById('models-note');
    note.style.display = 'block';
    note.textContent = t('model.fetching');
    // 允许用「刚填还没保存」的地址与 Key 试拉：不该逼用户先存一次
    const key = document.getElementById('key').value.trim();
    const body = { base_url: document.getElementById('base').value.trim() };
    if (key) body.api_key = key;
    const r = await api('/api/models', body);
    if (!r.ok) { note.textContent = t('model.fetch_failed') + (r.error || ''); return; }
    if (!r.models || !r.models.length) { note.textContent = t('model.fetch_empty'); return; }
    note.innerHTML = esc(t('model.pick_model')) + '<br>' + r.models.slice(0, 40)
      .map(m => `<a href="#" data-m="${esc(m)}" style="color:#7fd3ba;margin-right:10px">${esc(m)}</a>`).join('');
    note.querySelectorAll('a[data-m]').forEach(a => a.addEventListener('click', ev => {
      ev.preventDefault();
      document.getElementById('model').value = a.dataset.m;
    }));
  });
}

/* ------------------------------------------------------------------ 推送 */

const CHANNELS = [
  { id: 'onebot', name: 'OneBot 11（NapCat / Lagrange）', needs: ['endpoint', 'target', 'token'] },
  { id: 'wecom', name: '企业微信机器人（Webhook）', needs: ['endpoint'] },
  { id: 'wecom-aibot', name: '企业微信智能机器人（新版）',
    needs: ['endpoint', 'target', 'wecom_bot_id', 'wecom_bot_secret'] },
  { id: 'telegram', name: 'Telegram Bot', needs: ['tg_token', 'tg_chat'] },
  { id: 'dingtalk', name: '钉钉机器人', needs: ['endpoint', 'sign_secret', 'mobiles'] },
  { id: 'feishu', name: '飞书机器人', needs: ['endpoint', 'sign_secret'] },
  { id: 'discord', name: 'Discord Webhook', needs: ['endpoint'] },
  { id: 'slack', name: 'Slack Webhook', needs: ['endpoint'] },
  { id: 'bark', name: 'Bark（iOS 推送）', needs: ['server', 'bark_key'] },
  { id: 'ntfy', name: 'ntfy（可自建）', needs: ['server', 'topic', 'ntfy_token'] },
  { id: 'pushplus', name: 'PushPlus（推微信）', needs: ['pp_token'] },
  { id: 'qq', name: 'QQ 官方机器人', needs: ['target', 'qq_app_id', 'qq_app_secret'] },
  { id: 'serverchan', name: 'Server 酱', needs: ['token'] },
  { id: 'webhook', name: '通用 Webhook', needs: ['endpoint', 'token'] },
  { id: 'wechat-personal', name: '个人微信（第三方协议）', needs: ['endpoint', 'target', 'token'] },
];

async function renderPush() {
  const cfg = await api('/api/config/push');
  state.push = cfg;
  if (!cfg.ok) { view.innerHTML = `<div class="empty">${esc(cfg.error)}</div>`; return; }

  const opts = CHANNELS.map(c =>
    `<option value="${c.id}" ${c.id === cfg.provider ? 'selected' : ''}>${esc(c.name)}</option>`).join('');

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('push.title'))}</h2>
    <p class="hint">${t('push.hint')}</p>

    <label class="field"><span>${esc(t('push.channel'))}</span><select id="prov">${opts}</select></label>

    <div id="dyn"></div>

    <div class="row wrap">
      <button class="btn primary" id="btn-save">${esc(t('common.save'))}</button>
      <button class="btn" id="btn-test">${esc(t('push.test'))}</button>
      <button class="btn ghost sm" id="btn-detect">${esc(t('push.detect'))}</button>
      <span class="spacer"></span>
    </div>

    <div class="note" id="test-note" style="display:none"></div>
    <div class="note" id="detect-note" style="display:none"></div>
    <div class="note">${t('push.note')}</div>
  </div>

  <!-- 机器人卡片单独挂一个容器：装/起/停之后只需要重画这一块，
       整页重绘会把用户已经填了一半的表单冲掉。 -->
  <div id="bot-host"></div>`;

  const dyn = document.getElementById('dyn');
  const drawFields = () => {
    const id = document.getElementById('prov').value;
    const ch = CHANNELS.find(c => c.id === id) || { needs: [] };
    const has = n => ch.needs.includes(n);
    // 每个渠道各记一份「地址 + 目标」（后端存在 push-profiles.yaml）。
    // 不能直接用 cfg.endpoint / cfg.target —— 那两个字段是所有渠道共用的，
    // 切换渠道时会看到别人的值，用户会以为自己的配置被改了。
    const saved = (cfg.profiles && cfg.profiles[id]) || {};
    const ep = saved.endpoint !== undefined && saved.endpoint !== ''
      ? saved.endpoint
      : (id === cfg.provider ? cfg.endpoint : '');
    const tg = saved.target !== undefined && saved.target !== ''
      ? saved.target
      : (id === cfg.provider ? cfg.target : '');
    // 字段名是复用的（endpoint / token / target 三个），但每个渠道叫法不同 ——
    // 所以按 needs 决定渲染哪个位置、写什么标签。
    const F = {
      endpoint: { id: 'endpoint', key: 'push.f_webhook', ph: 'https://…',
                  val: ep, badge: '' },
      // OneBot 的地址是「我们自己起个 HTTP 服务，把地址给它」，不是 Webhook
      onebot: { id: 'endpoint', key: 'push.onebot_url', ph: 'http://127.0.0.1:3000',
                val: ep, badge: '' },
      server: { id: 'endpoint', key: 'push.f_server', ph: 'push.f_server_ph',
                val: ep, badge: '' },
      token: { id: 'token', key: 'push.token', ph: 'push.token_ph',
               val: '', badge: cfg.token_set ? t('model.key_set') : t('push.token_unset'), pw: true },
      sign_secret: { id: 'token', key: 'push.f_sign_secret', ph: 'push.f_sign_secret_ph',
                     val: '', badge: cfg.token_set ? t('model.key_set') : t('push.f_optional'), pw: true },
      tg_token: { id: 'token', key: 'push.f_tg_token', ph: 'push.f_tg_token_ph',
                  val: '', badge: cfg.token_set ? t('model.key_set') : '', pw: true },
      bark_key: { id: 'token', key: 'push.f_bark_key', ph: 'push.f_bark_key_ph',
                  val: '', badge: cfg.token_set ? t('model.key_set') : '', pw: true },
      ntfy_token: { id: 'token', key: 'push.f_ntfy_token', ph: 'push.f_optional',
                    val: '', badge: cfg.token_set ? t('model.key_set') : t('push.f_optional'), pw: true },
      pp_token: { id: 'token', key: 'push.f_pp_token', ph: 'push.f_pp_token_ph',
                  val: '', badge: cfg.token_set ? t('model.key_set') : '', pw: true },
      // ttype: 只有「群号 / 用户号」这种目标才需要「群 / 私聊」下拉
      target: { id: 'target', key: 'push.target', ph: 'push.target_ph',
                val: tg, badge: '', ttype: true },
      tg_chat: { id: 'target', key: 'push.f_tg_chat', ph: 'push.f_tg_chat_ph',
                 val: tg, badge: '' },
      mobiles: { id: 'target', key: 'push.f_mobiles', ph: 'push.f_mobiles_ph',
                 val: tg, badge: '' },
      topic: { id: 'target', key: 'push.f_topic', ph: 'push.f_topic_ph',
               val: tg, badge: '' },
    };
    const field = (f) => {
      const badge = f.badge ? ` <span class="tag ok">${esc(f.badge)}</span>` : '';
      const ph = f.ph.includes('.') ? t(f.ph) : f.ph;
      const type = f.pw ? 'password' : 'text';
      // 两个渠道都需要「帮我把会话 id 找出来」：
      // Telegram 是 getUpdates，企微智能机器人是连 WS 监听一阵
      const btn = f.id === 'target' && (has('tg_chat') || id === 'wecom-aibot')
        ? `<button class="btn ghost sm" id="btn-chats">${esc(t('push.f_tg_discover'))}</button>` : '';
      const ttype = f.ttype
        ? `<select id="ttype" style="width:130px">
             <option value="group" ${cfg.target_type === 'group' ? 'selected' : ''}>${esc(t('push.group'))}</option>
             <option value="private" ${cfg.target_type === 'private' ? 'selected' : ''}>${esc(t('push.private'))}</option>
           </select>` : '';
      return `<label class="field"><span>${esc(t(f.key))}${badge}</span>
        <div class="row">
          <input type="${type}" id="${f.id}" value="${esc(f.val)}" placeholder="${esc(ph)}">
          ${ttype}${btn}
        </div></label>`;
    };

    // 用数组拼，不要写成一长串三元 + `+`：
    // `?:` 的优先级比 `+` 低，那样写会被解析成 a ? b : ('' + c ? d : …)，
    // 逻辑全串，而且错得很安静。
    const parts = [];
    if (has('wecom_bot_id')) {
      parts.push(`<label class="field"><span>${esc(t('push.aibot_id'))} ${
        cfg.wecom_bot_set ? `<span class="tag ok">${esc(t('model.key_set'))}</span>` : ''}</span>
        <input type="text" id="wecom_bot_id" placeholder="${esc(t('push.aibot_id_ph'))}"></label>`);
    }
    if (has('wecom_bot_secret')) {
      parts.push(`<label class="field"><span>${esc(t('push.aibot_secret'))} ${
        cfg.wecom_secret_set ? `<span class="tag ok">${esc(t('model.key_set'))}</span>` : ''}</span>
        <input type="password" id="wecom_bot_secret" placeholder="${esc(t('push.aibot_secret'))}"></label>`);
    }
    if (has('qq_app_id')) {
      parts.push(`<label class="field"><span>${esc(t('push.appid'))} ${
        cfg.qq_appid_set ? `<span class="tag ok">${esc(t('model.key_set'))}</span>` : ''}</span>
        <input type="text" id="qq_app_id" placeholder="${esc(t('push.appid'))}"></label>`);
    }
    if (has('qq_app_secret')) {
      parts.push(`<label class="field"><span>${esc(t('push.secret'))} ${
        cfg.qq_secret_set ? `<span class="tag ok">${esc(t('model.key_set'))}</span>` : ''}</span>
        <input type="password" id="qq_app_secret" placeholder="${esc(t('push.secret'))}"></label>`);
    }
    // 顺序按「最该先填的排前面」：地址 → 凭据 → 目标
    if (has('server')) parts.push(field(F.server));
    else if (has('endpoint')) parts.push(field(id === 'onebot' ? F.onebot : F.endpoint));
    if (has('token')) parts.push(field(F.token));
    if (has('sign_secret')) parts.push(field(F.sign_secret));
    if (has('tg_token')) parts.push(field(F.tg_token));
    if (has('bark_key')) parts.push(field(F.bark_key));
    if (has('ntfy_token')) parts.push(field(F.ntfy_token));
    if (has('pp_token')) parts.push(field(F.pp_token));
    if (has('tg_chat')) parts.push(field(F.tg_chat));
    else if (has('target')) parts.push(field(F.target));
    if (has('mobiles')) parts.push(field(F.mobiles));
    if (has('topic')) parts.push(field(F.topic));
    if (id === 'wecom-aibot') parts.push(`<div class="note">${t('push.aibot_note')}</div>`);
    dyn.innerHTML = parts.join('');

    const chatBtn = document.getElementById('btn-chats');
    if (chatBtn) chatBtn.addEventListener('click', discoverChats);
  };

  // 让程序去找会话 id，省得用户对着那串数字发懵。
  //
  // Telegram 一次调用就回来；企业微信智能机器人得连上 WS 听十几秒
  // （它是回调制，会话 id 只在别人说话时送过来）。后端把这两种都放到
  // 后台线程，所以这里统一是「POST 启动 → 轮询 GET 拿结果」。
  const renderChats = (r, note) => {
    if (r.error) {
      note.innerHTML = `<b style="color:#e05c5c">${esc(t('common.unknown'))}</b><br>` +
        `<code style="white-space:pre-wrap">${esc(r.error)}</code>`;
      return;
    }
    const chats = r.chats || [];
    let html = chats.length
      ? `<b style="color:#7fd3ba">${esc(t('push.f_tg_found'))}</b><br>` +
        chats.map(c => `· <code>${esc(c.id)}</code> ${esc(c.name)} ` +
          `<a href="#" data-chat="${esc(c.id)}" style="color:#7fd3ba">${esc(t('push.fill'))}</a>`
        ).join('<br>')
      : `${esc(t('push.f_tg_none'))}<br>${esc(t('push.f_tg_discover_hint'))}`;
    if (r.note) html += `<br><br>${esc(r.note)}`;
    // 认不出的帧原样贴出来：万一官方改了字段名，用户把这行发过来就能定位
    if ((r.unknown_frames || []).length) {
      html += `<br><br><b>${esc(t('push.f_unknown_frames'))}</b><br>` +
        r.unknown_frames.map(f =>
          `<code style="white-space:pre-wrap">${esc(f)}</code>`).join('<br>');
    }
    note.innerHTML = html;
    note.querySelectorAll('a[data-chat]').forEach(a => a.addEventListener('click', ev => {
      ev.preventDefault();
      const node = document.getElementById('target');
      if (node) node.value = a.dataset.chat;
    }));
  };

  const discoverChats = async () => {
    const note = document.getElementById('test-note');
    note.style.display = 'block';
    // 两个渠道的等待时间差一个数量级，文案别串台：
    // 企微要连上 WS 听十几秒，Telegram 一次调用就回来。
    const prov = document.getElementById('prov').value;
    note.textContent = prov === 'wecom-aibot'
      ? t('push.f_aibot_listen')
      : t('push.f_tg_discovering');
    const val = (id) => {
      const n = document.getElementById(id);
      return n ? n.value.trim() : '';
    };
    const start = await api('/api/push/discover', {
      provider: prov,
      token: val('token'),
      wecom_bot_id: val('wecom_bot_id'),
      wecom_bot_secret: val('wecom_bot_secret'),
    });
    if (!start.ok) {
      note.innerHTML = `<b style="color:#e05c5c">${esc(start.error || '')}</b>`;
      return;
    }
    for (let i = 0; i < 40; i++) {
      await new Promise(r => setTimeout(r, 1500));
      const st = await api('/api/push/discover');
      if (st.running) {
        note.textContent = `${start.message || ''}（${i + 1}）`;
        continue;
      }
      renderChats(st, note);
      return;
    }
    note.textContent = t('push.f_tg_timeout');
  };

  drawFields();
  document.getElementById('prov').addEventListener('change', drawFields);

  const collect = () => {
    const p = { provider: document.getElementById('prov').value };
    for (const f of ['endpoint', 'target', 'target_type', 'qq_app_id', 'qq_app_secret',
                     'wecom_bot_id', 'wecom_bot_secret', 'token']) {
      const node = document.getElementById(f);
      if (node && node.value.trim()) p[f] = node.value.trim();
    }
    return p;
  };

  document.getElementById('btn-save').addEventListener('click', async () => {
    const r = await api('/api/config/push', collect());
    if (r.ok) {
      toast(t('common.saved'), t('common.wrote_fields') + (r.wrote || []).join('、'));
      renderPush();
      refreshDots();
    } else {
      toast(t('common.save_failed'), r.error || t('common.unknown'), 'err');
    }
  });

  document.getElementById('btn-test').addEventListener('click', async () => {
    const note = document.getElementById('test-note');
    note.style.display = 'block';
    note.textContent = t('push.testing');
    const r = await api('/api/push/test', {});
    if (r.ok) {
      note.innerHTML = `<b style="color:#7fd3ba">${esc(t('push.test_ok'))}</b>` +
        `（${esc(t('push.msg_id'))} = ${esc(r.message_id || '-')}）。${esc(t('push.test_ok_msg'))}`;
      toast(t('push.test_ok'), t('push.test_ok_msg'));
    } else {
      note.innerHTML = `<b style="color:#e05c5c">${esc(t('push.test_fail'))}</b><br><code>${esc(r.error || t('common.unknown'))}</code>`;
      toast(t('push.test_fail'), r.error || '', 'err');
    }
  });

  // 检测本机有没有跑机器人服务：端口上有没有东西在听，是最有用的线索
  document.getElementById('btn-detect').addEventListener('click', async () => {
    const note = document.getElementById('detect-note');
    note.style.display = 'block';
    note.textContent = t('push.detecting');
    const r = await api('/api/detect-bot', {});
    if (!r.ok) { note.textContent = esc(r.error || ''); return; }
    const found = r.found || [];
    if (found.length) {
      note.innerHTML = `<b style="color:#7fd3ba">${esc(t('push.detect_found'))}</b><br>` +
        found.map(p =>
          `· <code>http://127.0.0.1:${p}</code> ` +
          `<a href="#" data-port="${p}" style="color:#7fd3ba">${esc(t('push.fill'))}</a>`
        ).join('<br>');
      note.querySelectorAll('a[data-port]').forEach(a => a.addEventListener('click', ev => {
        ev.preventDefault();
        const sel = document.getElementById('prov');
        if (sel) { sel.value = 'onebot'; drawFields(); }
        const ep = document.getElementById('endpoint');
        if (ep) ep.value = 'http://127.0.0.1:' + a.dataset.port;
      }));
    } else {
      note.innerHTML = `<b>${esc(t('push.detect_none'))}</b><br><br>` +
        `${esc(t('push.detect_install'))}<br>` +
        `<a href="${esc(r.napcat_url)}" target="_blank" style="color:#7fd3ba">${esc(r.napcat_url)}</a>` +
        `<br><br>${esc(t('push.napcat_hint'))}`;
    }
  });

  /* -------- QQ 机器人（NapCat）：装 / 起 / 停 / 看日志 -------- */
  // 为什么塞在推送页、不单开一页：用户是在「配推送」的时候才想到要装机器人，
  // 单独一个导航项只会让他多点一次，还得自己记得绕回来。
  const botHost = document.getElementById('bot-host');

  const botHtml = (b) => {
    const st = !b.installed
      ? `<span class="tag">${esc(t('bot.not_installed'))}</span>`
      : (b.running
        ? `<span class="tag ok">${esc(t('bot.running'))}${b.pid ? ' · pid ' + b.pid : ''}</span>`
        : `<span class="tag">${esc(t('bot.stopped'))}</span>`);
    // 进程活着不等于服务起好了：NapCat 要登录成功之后才会开 OneBot 的 HTTP 端口，
    // 所以「端口在听没有」才是真正能判断推送能不能用的信号。
    const svc = b.port_open
      ? `<span class="tag ok">${esc(t('bot.service_up'))} :${b.port}</span>`
      : `<span class="tag">${esc(t('bot.service_down'))}</span>`;

    const buttons = [];
    if (!b.installed) {
      buttons.push(`<button class="btn primary" data-bot="install">${esc(t('bot.install'))}</button>`);
    } else {
      buttons.push(b.running
        ? `<button class="btn" data-bot="stop">${esc(t('bot.stop'))}</button>` +
          `<button class="btn primary" data-bot="start">${esc(t('bot.restart'))}</button>`
        : `<button class="btn primary" data-bot="start">${esc(t('bot.start'))}</button>`);
      buttons.push(`<button class="btn ghost sm" data-bot="install">${esc(t('bot.reinstall'))}</button>`);
      buttons.push(`<button class="btn ghost sm" data-bot="opendir">${esc(t('bot.dir'))}</button>`);
    }
    const dlPage = b.download_page || '';
    if (dlPage) {
      buttons.push(`<a class="btn ghost sm" href="${esc(dlPage)}" target="_blank">`
        + `${esc(t('bot.download_page'))}</a>`);
    }

    const log = String(b.log || '').trim();
    return `
    <div class="card">
      <h2>${esc(t('bot.title'))}</h2>
      <p class="hint">${esc(t('bot.hint'))}</p>
      <table>
        <tr><th style="width:110px">${esc(t('bot.state'))}</th><td>${st} ${svc}</td></tr>
        <tr><th>${esc(t('bot.dir'))}</th><td><code>${esc(b.root || '')}</code>${
          b.flavor ? ' · ' + esc(b.flavor) : ''}</td></tr>
        <tr><th>${esc(t('bot.version'))}</th><td>${esc(b.version || '—')}</td></tr>
      </table>
      ${b.installed ? '' : `<label class="field" style="max-width:320px"><span>${esc(t('bot.source'))}</span>
        <select id="bot-src">
          <option value="auto">${esc(t('bot.source_auto'))}</option>
          ${(b.sources || []).map((s, i) =>
            `<option value="${i}">${esc(s.label)}</option>`).join('')}
        </select></label>`}
      <div class="row wrap" style="margin:12px 0">${buttons.join(' ')}</div>
      ${b.installing ? `<div class="note" style="display:block">${esc(t('bot.installing'))}</div>` : ''}
      ${b.install_error ? `<div class="note" style="display:block"><b style="color:#e05c5c">${
        esc(t('common.save_failed'))}</b><br><code style="white-space:pre-wrap">${
        esc(b.install_error)}</code></div>` : ''}
      <div class="note" id="bot-note" style="display:none"></div>
      <p class="hint" style="margin:12px 0 6px">${esc(t('bot.log'))}</p>
      <pre class="logbox">${esc(log || t('bot.log_empty'))}</pre>
      <p class="hint" style="margin-top:10px">${esc(t('bot.scan_hint'))}</p>
      <p class="hint">${esc(t('bot.manual'))}</p>
    </div>`;
  };

  const paintBot = async () => {
    const b = await api('/api/bot/napcat');
    if (!b.ok) { botHost.innerHTML = ''; return; }
    botHost.innerHTML = botHtml(b);
    // 安装是后端的**后台任务**（下载几十 MB，界面不能卡住等），
    // 所以没结束之前每 2 秒回来看一眼：装完自动变成「启动」按钮，
    // 用户不需要自己按刷新。
    if (b.installing) setTimeout(paintBot, 2000);
    botHost.querySelectorAll('button[data-bot]').forEach(btn =>
      btn.addEventListener('click', async () => {
        const act = btn.dataset.bot;
        if (act === 'opendir') { await api('/api/bot/napcat/open', {}); return; }
        const n = document.getElementById('bot-note');
        n.style.display = 'block';
        // 下载几十 MB 要等一会儿，先把「正在干活」摆出来，别让人以为按钮没反应
        n.textContent = act === 'install' ? t('bot.installing') : '…';
        btn.disabled = true;
        // 安装时把选中的下载源带上：校园网下「自动」要等前一个源超时才轮到镜像，
        // 而用户往往一开始就知道该走哪个。
        const src = document.getElementById('bot-src');
        const body = (act === 'install' && src) ? { source: src.value } : {};
        const r = await api(`/api/bot/napcat/${act}`, body);
        btn.disabled = false;
        if (!r.ok) {
          n.innerHTML = `<b style="color:#e05c5c">${esc(t('common.save_failed'))}</b><br>`
            + `<code style="white-space:pre-wrap">${esc(r.error || t('common.unknown'))}</code>`;
        } else if (r.message) {
          toast(t('bot.title'), r.message);
        }
        await paintBot();
      }));
  };
  await paintBot();
}

/* ---------------------------------------------------------------- 时间表 */

/** `HH:mm` 加若干分钟；填不出合法时间就原样返回。 */
function plusMin(hhmm, minutes) {
  const m = /^(\d{1,2}):(\d{2})$/.exec(String(hhmm).trim());
  if (!m) return hhmm;
  const total = (Number(m[1]) * 60 + Number(m[2]) + minutes + 1440) % 1440;
  return `${String(Math.floor(total / 60)).padStart(2, '0')}:${String(total % 60).padStart(2, '0')}`;
}

async function renderTimetable() {
  const d = await api('/api/timetable');
  state.timetable = d;
  if (!d.ok) { view.innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }

  const slots = d.slots || [];
  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('st.title'))}</h2>
    <p class="hint">${t('st.hint')}</p>

    <div class="row wrap" style="margin-bottom:12px">
      <button class="btn sm" id="btn-ci">${esc(t('st.import_ci'))}</button>
      <input type="file" id="ci-file" accept=".json,application/json" style="display:none">
      <span class="spacer"></span>
    </div>

    <div class="slot-row slot-head">
      <div>${esc(t('st.col_no'))}</div><div>${esc(t('st.col_start'))}</div>
      <div>${esc(t('st.col_end'))}</div><div>${esc(t('st.col_kind'))}</div>
      <div>${esc(t('st.col_name'))}</div><div></div>
    </div>
    <div id="slots"></div>
    <div class="row" style="margin-top:12px">
      <button class="btn sm" id="btn-add">${esc(t('st.add'))}</button>
      <span class="spacer"></span>
      <button class="btn primary" id="btn-save">${esc(t('common.save'))}</button>
    </div>
    <div class="note" id="win-note" style="display:none"></div>
  </div>`;

  const host = document.getElementById('slots');

  // 序号跟着顺序走：挪动/插入/删除之后必须重编，否则编号会与实际次序对不上
  const renumber = () => {
    [...host.children].forEach((r, i) => {
      r.firstElementChild.textContent = i + 1;
      // 第一行不能再上移、最后一行不能再下移（按钮直接置灰，比点了没反应清楚）
      r.querySelector('[data-op="up"]').disabled = i === 0;
      r.querySelector('[data-op="down"]').disabled = i === host.children.length - 1;
    });
  };

  const build = (s) => el(`<div class="slot-row">
      <div style="color:var(--text-dim2);font-family:var(--mono);font-size:12px;padding-top:8px">0</div>
      <input type="text" value="${esc(s.start || '08:00')}" placeholder="08:00">
      <input type="text" value="${esc(s.end || '08:45')}" placeholder="08:45">
      <select>
        <option value="class" ${s.kind === 'class' ? 'selected' : ''}>${esc(t('st.kind_class'))}</option>
        <option value="break" ${s.kind === 'break' ? 'selected' : ''}>${esc(t('st.kind_break'))}</option>
      </select>
      <input type="text" value="${esc(s.name || '')}" placeholder="${esc(t('st.name_ph'))}">
      <div class="row-ops">
        <button class="btn sm" data-op="up" title="${esc(t('st.op_up'))}">↑</button>
        <button class="btn sm" data-op="down" title="${esc(t('st.op_down'))}">↓</button>
        <button class="btn sm" data-op="ins" title="${esc(t('st.op_insert'))}">+</button>
        <button class="btn sm danger" data-op="del" title="${esc(t('st.op_del'))}">×</button>
      </div>
    </div>`);

  /** 插到 after 之后；after 为空则追加到末尾。 */
  const addRow = (s, after) => {
    const row = build(s);
    if (after) host.insertBefore(row, after.nextElementSibling);
    else host.appendChild(row);
    renumber();
    return row;
  };

  slots.forEach((s) => addRow(s));
  if (!slots.length) addRow({ start: '08:00', end: '08:45', kind: 'class', name: '' });

  // 事件委托：行是动态增删的，逐个绑监听既啰嗦又容易漏
  host.addEventListener('click', (e) => {
    const btn = e.target.closest('button[data-op]');
    if (!btn) return;
    const row = btn.closest('.slot-row');
    const op = btn.dataset.op;
    if (op === 'del') {
      row.remove();
      renumber();
    } else if (op === 'up') {
      const prev = row.previousElementSibling;
      if (prev) host.insertBefore(row, prev);
      renumber();
    } else if (op === 'down') {
      const next = row.nextElementSibling;
      if (next) host.insertBefore(next, row);
      renumber();
    } else if (op === 'ins') {
      // 新段接在上一段之后：开始时间取上一段的结束时间，默认再排 45 分钟。
      // 比给个固定的 08:00 更可能一次填对 —— 用户多半就是想在原基础上加一节。
      const ins = row.querySelectorAll('input');
      const sel = row.querySelector('select');
      const start = ins[1].value.trim() || '09:00';
      addRow({ start, end: plusMin(start, 45), kind: sel.value, name: '' }, row);
    }
  });

  document.getElementById('btn-add').addEventListener('click', () => addRow({}));

  const collect = () => [...host.children].map((r, i) => {
    const ins = r.querySelectorAll('input');
    const sel = r.querySelector('select');
    return {
      period: i + 1,
      start: ins[0].value.trim(),
      end: ins[1].value.trim(),
      kind: sel.value,
      name: ins[2].value.trim() || null,
    };
  });

  const showWindows = (w) => {
    const note = document.getElementById('win-note');
    note.style.display = 'block';
    note.innerHTML = (w && w.length)
      ? `<b>${esc(t('st.windows'))}</b><br>` + w.map(x =>
          `· ${esc(x.name)}（${esc(x.start)}–${esc(x.end)}，scope=${esc(x.scope)}）`).join('<br>')
      : `<b style="color:#d9a343">${esc(t('st.no_windows'))}</b>${esc(t('st.no_windows_why'))}`;
  };

  document.getElementById('btn-save').addEventListener('click', async () => {
    const payload = {
      timetables: [{
        id: 'default', name: d.name || 'default', is_active: true, source: 'manual',
        slots: collect(),
      }],
    };
    const r = await api('/api/timetable', payload);
    if (r.ok) {
      toast(t('common.saved'), String(r.slots));
      showWindows(r.windows);
    } else {
      toast(t('common.save_failed'), r.error || '', 'err');
    }
  });

  // --- ClassIsland 导入 ---
  // 浏览器拿不到文件真实路径（安全限制），只能读内容，所以整份 JSON 传给后端解析。
  // 后端那边是只读的，不会改 ClassIsland 的任何文件。
  document.getElementById('btn-ci').addEventListener('click', () => {
    document.getElementById('ci-file').click();
  });
  document.getElementById('ci-file').addEventListener('change', async (e) => {
    const f = e.target.files && e.target.files[0];
    if (!f) return;
    const note = document.getElementById('win-note');
    note.style.display = 'block';
    note.textContent = t('st.imported') + '…';
    const text = await f.text();
    const r = await api('/api/import/classisland', { json: text });
    e.target.value = '';
    if (!r.ok) {
      note.innerHTML = `<b style="color:#e05c5c">${esc(t('common.save_failed'))}</b><br><code>${esc(r.error || '')}</code>`;
      return;
    }
    toast(t('st.imported'), `${r.timetable_name || ''} · ${r.slots} / ${r.entries}`);
    await renderTimetable();
    showWindows(null);
    // renderTimetable() 重建了 DOM，note 得重新取一次。
    // 导入进来的课默认全部勾上「录制」，用户接下来要做的事正是「挑掉不想录的」，
    // 所以把提示摆好、再把人送过去，省得他自己在页面之间找。
    const n2 = document.getElementById('win-note');
    if (n2) {
      n2.style.display = 'block';
      n2.innerHTML =
        `<b>${esc(t('st.imported'))}</b> · ${esc(r.timetable_name || '')} ` +
        `<span style="color:var(--text-dim2)">${r.slots} / ${r.entries}</span><br>` +
        `${esc(t('st.import_hint'))}`;
    }
    setTimeout(() => go('schedule'), 1200);
  });
}

/* ---------------------------------------------------------------- 课表 */

const DAYS = [['Mon', 'day_mon'], ['Tue', 'day_tue'], ['Wed', 'day_wed'],
              ['Thu', 'day_thu'], ['Fri', 'day_fri'], ['Sat', 'day_sat'], ['Sun', 'day_sun']];
const CYCLES = [['every', 'cycle_every'], ['odd', 'cycle_odd'], ['even', 'cycle_even']];

async function renderSchedule() {
  const d = await api('/api/schedule');
  state.schedule = d;
  if (!d.ok) { view.innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }

  // 教师 id -> 姓名。ClassIsland 里没填老师名的科目，导入器会落成 unassigned；
  // 把这种内部 id 直接摆在表格里，用户只会问「unassigned 是什么意思」。
  // 所以界面上一律显示姓名，保存时再映射回 id。
  const nameOf = (id) => {
    if (!id || id === 'unassigned') return '';
    const hit = (d.teachers || []).find(x => x.id === id);
    return hit ? hit.name : id;
  };

  // 唯一的真数据源。两种模式都只改这个数组，所以切换视图天然互通，
  // 也不需要「切模式时重新读盘」这种补丁。
  let model = (d.entries || []).map(e => ({
    day: e.day || 'Mon',
    start: e.start || '08:00',
    end: e.end || '08:45',
    course: e.course || '',
    teacher: nameOf(e.teacherId),
    cycle: e.cycle || 'every',
    record: e.record !== false,
  }));
  let mode = 'subject';

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('sc.title'))}</h2>
    <p class="hint">${t('sc.hint')}</p>
    <div class="row wrap" style="margin-bottom:12px">
      <div class="seg" id="mode">
        <button class="seg-btn" data-mode="subject">${esc(t('sc.mode_subject'))}</button>
        <button class="seg-btn" data-mode="table">${esc(t('sc.mode_table'))}</button>
      </div>
      <button class="btn sm" id="btn-on">${esc(t('sc.all_on'))}</button>
      <button class="btn sm" id="btn-off">${esc(t('sc.all_off'))}</button>
      <span class="spacer"></span>
      <span style="font-size:12px;color:var(--text-dim2)" id="cnt"></span>
      <button class="btn sm" id="btn-add">${esc(t('sc.add'))}</button>
      <button class="btn sm" id="btn-csv">${esc(t('sc.export'))}</button>
      <button class="btn primary" id="btn-save">${esc(t('common.save'))}</button>
    </div>
    <div id="body"></div>
    <div class="note" id="save-note" style="display:none"></div>
  </div>`;

  const body = document.getElementById('body');
  const upd = () => {
    const n = model.filter(m => m.record).length;
    document.getElementById('cnt').textContent =
      `${t('sc.count')} ${model.length} ${t('sc.entries')} · ${t('sc.recording')} ${n}`;
    document.querySelectorAll('#mode .seg-btn').forEach(b =>
      b.classList.toggle('active', b.dataset.mode === mode));
  };

  /* ---- 模式一：按科目选。一个科目一行，适合「只录我自己的课」 ---- */
  const paintSubjects = () => {
    const names = [...new Set(model.map(m => m.course))];
    if (!names.length) {
      body.innerHTML = `<div class="empty">${esc(t('sc.empty'))}</div>`;
      return;
    }
    body.innerHTML = `<p class="hint" style="margin-bottom:10px">${esc(t('sc.by_subject_hint'))}</p>` +
      names.map(name => {
        const list = model.filter(m => m.course === name);
        return `
        <div class="slot-row" style="grid-template-columns:1fr 110px 130px">
          <div>${esc(name)}</div>
          <div style="color:var(--text-dim2);font-size:12px">${list.length} ${esc(t('sc.lessons'))}</div>
          <label class="check"><input type="checkbox" data-course="${esc(name)}"> ${esc(t('sc.record'))}</label>
        </div>`;
      }).join('');

    // 三态：全录 / 全不录 / 只录了一部分（indeterminate，一眼能看出不齐）
    body.querySelectorAll('input[data-course]').forEach(cb => {
      const list = model.filter(m => m.course === cb.dataset.course);
      const on = list.filter(m => m.record).length;
      cb.checked = on > 0;
      cb.indeterminate = on > 0 && on < list.length;
      cb.addEventListener('change', () => {
        const v = cb.checked;
        model.forEach(m => { if (m.course === cb.dataset.course) m.record = v; });
        paint();
      });
    });
  };

  /* ---- 模式二：逐条编辑。列与时间表页对齐，多了「录制」与「科目」 ---- */
  const COLS = '92px 74px 74px 1fr 120px 68px 58px 136px';
  const paintTable = () => {
    body.innerHTML = `
      <div class="slot-row slot-head" style="grid-template-columns:${COLS}">
        <div>${esc(t('sc.col_day'))}</div>
        <div>${esc(t('sc.col_start'))}</div>
        <div>${esc(t('sc.col_end'))}</div>
        <div>${esc(t('sc.col_course'))}</div>
        <div>${esc(t('sc.col_teacher'))}</div>
        <div>${esc(t('sc.col_cycle'))}</div>
        <div>${esc(t('sc.col_record'))}</div>
        <div></div>
      </div>` +
      model.map((m, i) => `
      <div class="slot-row" data-i="${i}" style="grid-template-columns:${COLS}">
        <select data-k="day">${DAYS.map(([v, k]) =>
          `<option value="${v}" ${m.day === v ? 'selected' : ''}>${esc(t('sc.' + k))}</option>`).join('')}</select>
        <input type="text" data-k="start" value="${esc(m.start)}">
        <input type="text" data-k="end" value="${esc(m.end)}">
        <input type="text" data-k="course" value="${esc(m.course)}" placeholder="${esc(t('sc.course_ph'))}">
        <input type="text" data-k="teacher" value="${esc(m.teacher)}" placeholder="${esc(t('sc.teacher_unset'))}">
        <select data-k="cycle" style="width:74px">${CYCLES.map(([v, k]) =>
          `<option value="${v}" ${m.cycle === v ? 'selected' : ''}>${esc(t('sc.' + k))}</option>`).join('')}</select>
        <label class="check" title="${esc(t('sc.record_hint'))}">
          <input type="checkbox" data-k="record" ${m.record ? 'checked' : ''}>
        </label>
        <div class="row-ops">
          <button class="btn sm" data-op="up" ${i === 0 ? 'disabled' : ''}
                  title="${esc(t('st.op_up'))}">↑</button>
          <button class="btn sm" data-op="down" ${i === model.length - 1 ? 'disabled' : ''}
                  title="${esc(t('st.op_down'))}">↓</button>
          <button class="btn sm" data-op="ins" title="${esc(t('st.op_insert'))}">+</button>
          <button class="btn sm danger" data-op="del" title="${esc(t('st.op_del'))}">×</button>
        </div>
      </div>`).join('');

    body.querySelectorAll('.slot-row[data-i]').forEach(row => {
      const i = Number(row.dataset.i);
      row.querySelectorAll('[data-k]').forEach(node => {
        const k = node.dataset.k;
        if (k === 'record') {
          node.addEventListener('change', () => { model[i].record = node.checked; upd(); });
        } else {
          // 每次击键都同步进 model：切模式之前不需要额外「保存草稿」这一步
          node.addEventListener('input', () => { model[i][k] = node.value; });
          node.addEventListener('change', () => { model[i][k] = node.value; });
        }
      });
      // 顺序调整：直接改 model 数组再重绘。
      // 之所以敢重绘，是因为 model 已经提出来了（不是在 DOM 里就地改）——
      // 重绘后绑定的监听、输入框内容全都跟着重建，不会出现半新半旧的状态。
      row.querySelectorAll('button[data-op]').forEach(b =>
        b.addEventListener('click', () => {
          const op = b.dataset.op;
          if (op === 'del') {
            model.splice(i, 1);
          } else if (op === 'up' && i > 0) {
            [model[i - 1], model[i]] = [model[i], model[i - 1]];
          } else if (op === 'down' && i < model.length - 1) {
            [model[i + 1], model[i]] = [model[i], model[i + 1]];
          } else if (op === 'ins') {
            // 插一条与当前行同科目的空条目：接着上一节往下排最省事
            model.splice(i + 1, 0, {
              ...model[i],
              start: model[i].end || '09:00',
              end: plusMin(model[i].end || '09:00', 45),
              teacher: model[i].teacher,
              record: model[i].record,
            });
          } else {
            return;
          }
          paint();
        }));
    });
  };

  const paint = () => {
    if (mode === 'subject') paintSubjects(); else paintTable();
    upd();
  };

  document.querySelectorAll('#mode .seg-btn').forEach(b =>
    b.addEventListener('click', () => { mode = b.dataset.mode; paint(); }));

  document.getElementById('btn-on').addEventListener('click', () => {
    model.forEach(m => { m.record = true; });
    paint();
  });
  document.getElementById('btn-off').addEventListener('click', () => {
    model.forEach(m => { m.record = false; });
    paint();
  });

  document.getElementById('btn-add').addEventListener('click', () => {
    model.push({
      day: 'Mon', start: '08:00', end: '08:45', course: '', teacher: '',
      cycle: 'every', record: true,
    });
    // 新加的行在科目视图里会立刻变成一个（还没名字的）科目，那没意义 ——
    // 直接切到大表，让用户就地填完。
    mode = 'table';
    paint();
  });

  document.getElementById('btn-csv').addEventListener('click', () => {
    const rows = [['day', 'period', 'start', 'end', 'course', 'teacher', 'cycle', 'record'].join(',')];
    model.forEach((m, i) => {
      rows.push([m.day, i + 1, m.start, m.end, m.course, m.teacher, m.cycle, m.record]
        .map(v => String(v).replace(/,/g, '，')).join(','));
    });
    const blob = new Blob(['\ufeff' + rows.join('\n')], { type: 'text/csv;charset=utf-8' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'schedule.csv';
    a.click();
    toast(t('sc.exported'), t('sc.exported_d'));
  });

  document.getElementById('btn-save').addEventListener('click', async () => {
    // 姓名 -> id：沿用已有教师；输入了新名字就当场建一个。
    // 名字留空 = 未指定（unassigned），校验那边会提醒，但**不拦保存**。
    const teachers = (d.teachers || []).map(x => ({ ...x }));
    const idOf = (name) => {
      const n = String(name || '').trim();
      if (!n) return 'unassigned';
      const hit = teachers.find(x => x.name === n);
      if (hit) return hit.id;
      let k = teachers.length + 1;
      while (teachers.some(x => x.id === 't' + k)) k++;
      const id = 't' + k;
      teachers.push({ id, name: n, profile: id });
      return id;
    };

    const entries = model.map((m, i) => ({
      day: m.day,
      period: i + 1,
      start: String(m.start || '').trim(),
      end: String(m.end || '').trim(),
      course: String(m.course || '').trim() || '未命名课程',
      teacherId: idOf(m.teacher),
      record: !!m.record,
      cycle: m.cycle,
    }));

    const r = await api('/api/schedule', {
      teachers,
      week_template: { cycle: 'every', entries },
      weekend_template: { source: 'new', entries: [] },
      overrides: [],
    });
    const note = document.getElementById('save-note');
    note.style.display = 'block';
    if (r.ok) {
      toast(t('common.saved'), String(r.entries));
      // 保存会重建教师名单（新名字会被分配 id），回读一遍让界面与磁盘一致
      const back = await api('/api/schedule');
      if (back.ok) {
        state.schedule = back;
        const names = new Map((back.teachers || []).map(x => [x.id, x.name]));
        model = (back.entries || []).map(e => ({
          day: e.day, start: e.start, end: e.end, course: e.course,
          teacher: e.teacherId === 'unassigned' ? '' : (names.get(e.teacherId) || ''),
          cycle: e.cycle || 'every', record: e.record !== false,
        }));
        paint();
      }
      note.innerHTML = (r.issues && r.issues.length)
        ? `<b style="color:#d9a343">${esc(t('sc.issues'))}</b><br>· ` + r.issues.map(esc).join('<br>· ')
        : `<b style="color:#7fd3ba">${esc(t('sc.ok'))}</b>${esc(t('sc.ok_d'))}`;
    } else {
      toast(t('common.save_failed'), r.error || '', 'err');
      note.innerHTML = `<b style="color:#e05c5c">${esc(t('common.save_failed'))}</b><br>` +
        `<code>${esc(r.error || '')}</code>`;
    }
  });

  paint();
}

/* ---------------------------------------------------------- 录制与文件 */

async function renderRecord() {
  const d = await api('/api/config/general');
  if (!d.ok) { view.innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('rc.title'))}</h2>
    <p class="hint">${t('rc.hint')}</p>

    <div class="grid">
      <label class="field"><span>${esc(t('rc.height'))}</span>
        <input type="number" id="height" value="${d.record.height}"></label>
      <label class="field"><span>${esc(t('rc.fps'))}</span>
        <input type="number" id="fps" value="${d.record.fps}"></label>
      <label class="field"><span>${esc(t('rc.shot'))}</span>
        <input type="number" id="shot" value="${d.record.screenshot_interval_secs}"></label>
      <label class="field"><span>${esc(t('rc.ret'))}</span>
        <input type="number" id="ret" value="${d.cleanup.retention_hours}"></label>
    </div>

    <label class="field"><span>${esc(t('rc.audio'))}</span>
      <select id="audio">
        <option value="both" ${d.record.audio_source === 'both' ? 'selected' : ''}>${esc(t('rc.audio_both'))}</option>
        <option value="system" ${d.record.audio_source === 'system' ? 'selected' : ''}>${esc(t('rc.audio_sys'))}</option>
        <option value="mic" ${d.record.audio_source === 'mic' ? 'selected' : ''}>${esc(t('rc.audio_mic'))}</option>
      </select>
    </label>

    <label class="field"><span>${esc(t('rc.overlay'))}</span>
      <label class="check">
        <input type="checkbox" id="ov" ${d.overlay.enabled ? 'checked' : ''}> ${esc(t('rc.overlay_label'))}
      </label>
    </label>

    <label class="field"><span>${esc(t('rc.overlay_text'))}</span>
      <input type="text" id="ovtext" value="${esc(d.overlay.text)}"></label>

    <label class="field"><span>${esc(t('rc.tray'))}</span>
      <label class="check">
        <input type="checkbox" id="tray" ${d.ui && d.ui.tray_icon ? 'checked' : ''}> ${esc(t('rc.tray_label'))}
      </label>
    </label>

    <label class="field"><span>${esc(t('rc.lan'))}</span>
      <label class="check">
        <input type="checkbox" id="lan" ${d.ui && d.ui.allow_lan ? 'checked' : ''}> ${esc(t('rc.lan_label'))}
      </label>
      <p class="hint" style="margin:6px 0 0">${esc(t('rc.lan_hint'))}</p>
      ${d.ui && d.ui.allow_lan && d.push && d.push.preview_base
        ? `<p class="hint" style="margin:6px 0 0">${esc(t('rc.lan_url'))}
             <code>${esc(d.push.preview_base)}</code></p>` : ''}
    </label>

    <label class="field"><span>${esc(t('rc.image_card'))}</span>
      <label class="check">
        <input type="checkbox" id="imgcard" ${d.push && d.push.image_card ? 'checked' : ''}> ${esc(t('rc.image_card_label'))}
      </label>
      <p class="hint" style="margin:6px 0 0">${esc(t('rc.image_card_hint'))}</p>
    </label>

    <div class="row">
      <button class="btn primary" id="btn-save">${esc(t('common.save'))}</button>
      <span class="spacer"></span>
    </div>
  </div>

  <div class="card">
    <h2>${esc(t('rc.clean_title'))}</h2>
    <p class="hint">${t('rc.clean_hint')}</p>
    <div class="row">
      <button class="btn" id="btn-clean">${esc(t('rc.clean_preview'))}</button>
      <span class="spacer"></span>
    </div>
    <div class="note" id="clean-note" style="display:none"></div>
  </div>`;

  document.getElementById('btn-save').addEventListener('click', async () => {
    const payload = {
      height: +document.getElementById('height').value,
      fps: +document.getElementById('fps').value,
      screenshot_interval_secs: +document.getElementById('shot').value,
      retention_hours: +document.getElementById('ret').value,
      audio_source: document.getElementById('audio').value,
      overlay_enabled: document.getElementById('ov').checked,
      overlay_text: document.getElementById('ovtext').value,
      tray_icon: document.getElementById('tray').checked,
      allow_lan: document.getElementById('lan').checked,
      image_card: document.getElementById('imgcard').checked,
    };
    const r = await api('/api/config/general', payload);
    if (r.ok) {
      toast(t('common.saved'), t('common.wrote_fields') + (r.wrote || []).join('、'));
      // 局域网访问只写配置不会立刻生效 —— 绑定是在服务启动时做的，
      // 不说一句用户会以为开关坏了
      if ((r.wrote || []).includes('allow_lan')) {
        toast(t('rc.lan'), t('rc.lan_restart'));
      }
    } else {
      toast(t('common.save_failed'), r.error || '', 'err');
    }
  });

  document.getElementById('btn-clean').addEventListener('click', async () => {
    const note = document.getElementById('clean-note');
    note.style.display = 'block';
    note.textContent = t('rc.scanning');
    const r = await api('/api/clean/preview', {});
    if (!r.ok) { note.textContent = esc(r.error || ''); return; }
    note.innerHTML = esc(t('rc.clean_result', { s: r.scanned, d: r.deleting, k: r.skipped })) +
      (r.deleting ? `<br><span style="color:#d9a343">${esc(t('rc.clean_warn'))}</span>` : '');
  });
}

/* ---------------------------------------------------------------- 作业 */

async function renderJobs() {
  const d = await api('/api/jobs');
  if (!d.ok) { view.innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }
  const run = await api('/api/jobs/run');

  const rows = (d.jobs || []).map(j => `<tr>
      <td>${esc(j.date)}</td>
      <td class="mono-dim">${esc(j.course)}</td>
      <td><span class="tag ${j.state === 'PUSHED' ? 'ok' : ''}">${esc(j.state)}</span></td>
      <td class="mono-dim" style="font-size:11.5px">${esc(j.docx ? j.docx.split('\\').pop() : '—')}</td>
      <td class="mono-dim">${esc(j.last_error || '')}</td>
      <td style="text-align:right">${j.state === 'PUSHED'
        ? '' : `<button class="btn sm" data-run="${esc(j.id)}">${esc(t('jb.run_now'))}</button>`}</td>
    </tr>`).join('');

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('jb.title'))}</h2>
    <p class="hint">${t('jb.hint')}</p>
    <p class="hint">${esc(t('jb.run_hint'))}</p>
    <div class="note" id="run-note" style="display:${run.running ? 'block' : 'none'}">${
      run.running ? esc(t('jb.running')) : ''}</div>
    <table>
      <thead><tr>
        <th>${esc(t('jb.date'))}</th><th>${esc(t('jb.course'))}</th>
        <th>${esc(t('jb.state'))}</th><th>${esc(t('jb.doc'))}</th><th>${esc(t('jb.note'))}</th>
        <th></th>
      </tr></thead>
      <tbody>${rows}</tbody>
    </table>
    ${rows ? '' : `<div class="empty">${esc(t('jb.empty'))}</div>`}
  </div>`;

  // 「立即处理」：跳过处理窗口与错峰等待，现在就跑。
  // 后端在后台线程做，所以这里轮询状态 —— 一条流水线要跑几分钟。
  const poll = async () => {
    const st = await api('/api/jobs/run');
    const note = document.getElementById('run-note');
    if (!note) return;
    if (st.running) {
      note.style.display = 'block';
      note.textContent = t('jb.running');
      setTimeout(poll, 2000);
      return;
    }
    if (st.error) {
      note.style.display = 'block';
      note.innerHTML = `<b style="color:#e05c5c">${esc(t('jb.run_failed'))}</b><br>` +
        `<code style="white-space:pre-wrap">${esc(st.error)}</code>`;
      return;
    }
    if (st.message) {
      note.style.display = 'block';
      note.innerHTML = `<b style="color:#7fd3ba">${esc(st.message)}</b>`;
      // 跑完刷新一次列表，状态就变了
      setTimeout(() => { if (current === 'jobs') renderJobs(); }, 800);
    }
  };
  if (run.running) poll();

  view.querySelectorAll('button[data-run]').forEach(b =>
    b.addEventListener('click', async () => {
      const note = document.getElementById('run-note');
      note.style.display = 'block';
      note.textContent = t('jb.starting');
      b.disabled = true;
      const r = await api('/api/jobs/run', { id: b.dataset.run });
      if (!r.ok) {
        note.innerHTML = `<b style="color:#e05c5c">${esc(r.error || t('common.unknown'))}</b>`;
        b.disabled = false;
        return;
      }
      poll();
    }));
}

/* ---------------------------------------------------------------- 路由 */

const PAGES = {
  overview: ['nav.overview', renderOverview],
  run: ['nav.run', renderRun],
  model: ['nav.model', renderModel],
  push: ['nav.push', renderPush],
  timetable: ['nav.timetable', renderTimetable],
  schedule: ['nav.schedule', renderSchedule],
  record: ['nav.record', renderRecord],
  jobs: ['nav.jobs', renderJobs],
};

let current = 'overview';

async function go(page) {
  current = page;
  const [titleKey, fn] = PAGES[page] || PAGES.overview;
  titleEl.textContent = t(titleKey);
  document.querySelectorAll('.nav-item').forEach(b =>
    b.classList.toggle('active', b.dataset.page === page));
  view.innerHTML = `<div class="empty">${esc(t('common.loading'))}</div>`;
  try {
    await fn();
  } catch (e) {
    view.innerHTML = `<div class="empty">${esc(t('common.error_prefix'))}${esc(e.message)}</div>`;
  }
}

async function refreshDots() {
  try {
    const s = await api('/api/status');
    if (s.ok) { state.status = s; paintDots(s); }
  } catch { /* 状态刷不出来不影响主流程 */ }
}

document.getElementById('nav').addEventListener('click', e => {
  const btn = e.target.closest('.nav-item');
  if (btn) go(btn.dataset.page);
});
document.getElementById('btn-refresh').addEventListener('click', () => go(current));

// 退出：关窗口 ≠ 退程序（窗口是 --app 拉起的独立进程，后台服务还在跑），
// 所以必须给一个明确的出口，并且把这件事说清楚。
document.getElementById('btn-quit').addEventListener('click', async () => {
  if (!confirm(t('common.quit_body') + '\n\n' + t('common.quit_note'))) return;
  await api('/api/quit', {});
  document.body.innerHTML =
    `<div style="padding:80px;text-align:center;color:#8f8f8f;font-family:Segoe UI,sans-serif">` +
    `<div style="font-size:16px;color:#ededed">${esc(t('common.quit_done'))}</div>` +
    `<div style="margin-top:8px">${esc(t('common.quit_done_note'))}</div></div>`;
});

/* ---------------------------------------------------------------- 启动 */

(async function boot() {
  try {
    const r = await api('/api/i18n');
    if (r.ok) L = r.strings || {};
    document.documentElement.lang = r.lang || 'zh-CN';
  } catch { /* 拿不到就退回 key 本身，至少不会白屏 */ }
  applyStatic();
  paintBrand();
  go('overview');
  refreshDots();
})();
