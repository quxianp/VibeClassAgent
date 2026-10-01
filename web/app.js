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

// 渠道清单。
//
// 本阶段（QQ 侧优先）只保障前两个：个人账号机器人（OneBot）与 QQ 官方机器人。
// 其余渠道**代码完整保留**（点开能填、能保存），但在界面上标为「暂不可用」——
// 用户明确要求「暂时搁置微信端与外网端，但不删代码，要备注」。
// off: true 就是那条备注，不是禁用。
// 推送渠道。
//
// 只保留三类（用户 2026-09 的定向改造要求）：
//   1) QQ 个人账号机器人  ← OneBot 11，也就是 NapCat / Lagrange / go-cqhttp 那些「第三方协议实现」
//   2) QQ 官方机器人
//   3) 微信侧             ← 折叠进二级菜单，标「暂不可用」，但代码完整保留
//
// 前两类第一项**同时**覆盖了「个人账号机器人」与「第三方协议实现」——
// 在代码里它们本来就是同一个 Provider（见 push.rs 的 Provider::OneBot 注释），
// 所以这里不需要再拆成两个。
// off: true 表示「折叠 + 暂不可用」，不是禁用。
const CHANNELS = [
  { id: 'onebot', name: 'QQ 个人账号机器人（OneBot 11 / NapCat）',
    needs: ['endpoint', 'target', 'token'] },
  { id: 'qq', name: 'QQ 官方机器人', needs: ['target', 'qq_app_id', 'qq_app_secret'] },

  { id: 'wecom', name: '企业微信机器人（Webhook）', needs: ['endpoint'], off: true },
  { id: 'wecom-aibot', name: '企业微信智能机器人（新版）',
    needs: ['endpoint', 'target', 'wecom_bot_id', 'wecom_bot_secret'], off: true },
  { id: 'wechat-personal', name: '个人微信（第三方协议）',
    needs: ['endpoint', 'target', 'token'], off: true },
];

async function renderPush() {
  const cfg = await api('/api/config/push');
  state.push = cfg;
  if (!cfg.ok) { view.innerHTML = `<div class="empty">${esc(cfg.error)}</div>`; return; }

  // 二级菜单：QQ 侧可用、微信侧折叠并标「暂不可用」。
  // 用原生 optgroup —— 它本来就是分组控件，不必自己写一套折叠。
  const opts = [
    { label: t('push.group_qq'), items: CHANNELS.filter(c => !c.off) },
    { label: t('push.group_wechat') + '（' + t('push.unavailable') + '）',
      items: CHANNELS.filter(c => c.off) },
  ].map(g => `<optgroup label="${esc(g.label)}">` + g.items.map(c =>
      `<option value="${c.id}" ${c.id === cfg.provider ? 'selected' : ''}>${
        esc(c.name)}</option>`).join('') + '</optgroup>').join('');

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
      token: { id: 'token', key: 'push.token', ph: 'push.token_ph',
               val: '', badge: cfg.token_set ? t('model.key_set') : t('push.token_unset'), pw: true },
      // ttype: 只有「群号 / 用户号」这种目标才需要「群 / 私聊」下拉
      target: { id: 'target', key: 'push.target', ph: 'push.target_ph',
                val: tg, badge: '', ttype: true },
      mobiles: { id: 'target', key: 'push.f_mobiles', ph: 'push.f_mobiles_ph',
                 val: tg, badge: '' },
      topic: { id: 'target', key: 'push.f_topic', ph: 'push.f_topic_ph',
               val: tg, badge: '' },
    };
    const field = (f) => {
      const badge = f.badge ? ` <span class="tag ok">${esc(f.badge)}</span>` : '';
      const ph = f.ph.includes('.') ? t(f.ph) : f.ph;
      const type = f.pw ? 'password' : 'text';
      // 企微智能机器人需要「帮我把会话 id 找出来」：
      const btn = f.id === 'target' && id === 'wecom-aibot'
        ? `<button class="btn ghost sm" id="btn-chats">${esc(t('push.f_discover'))}</button>` : '';
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
    // 选到暂不可用的渠道时，先把话说清楚 —— 否则用户填了半天才发现用不了
    const chOff = CHANNELS.find(c => c.id === id);
    if (chOff && chOff.off) {
      parts.push(`<div class="note" style="display:block">
        <b style="color:#d9a343">${esc(t('push.unavailable'))}</b> —— ${esc(t('push.scope_note'))}
        <br><br>${esc(t('push.scope_now'))}</div>`);
    }
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
    if (has('endpoint')) parts.push(field(id === 'onebot' ? F.onebot : F.endpoint));
    if (has('token')) parts.push(field(F.token));
    if (has('target')) parts.push(field(F.target));
    if (has('mobiles')) parts.push(field(F.mobiles));
    if (has('topic')) parts.push(field(F.topic));
    if (id === 'wecom-aibot') parts.push(`<div class="note">${t('push.aibot_note')}</div>`);
    dyn.innerHTML = parts.join('');

    const chatBtn = document.getElementById('btn-chats');
    if (chatBtn) chatBtn.addEventListener('click', discoverChats);
  };

  // 让程序去找会话 id，省得用户对着那串数字发懵。
  //
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
      ? `<b style="color:#7fd3ba">${esc(t('push.f_discover_found'))}</b><br>` +
        chats.map(c => `· <code>${esc(c.id)}</code> ${esc(c.name)} ` +
          `<a href="#" data-chat="${esc(c.id)}" style="color:#7fd3ba">${esc(t('push.fill'))}</a>`
        ).join('<br>')
      : `${esc(t('push.f_discover_none'))}<br>${esc(t('push.f_discover_hint'))}`;
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
      const prov = document.getElementById('prov').value;
    note.textContent = prov === 'wecom-aibot'
      ? t('push.f_aibot_listen')
      : t('push.f_discover_ing');
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
    note.textContent = t('push.f_discover_timeout');
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

    // 两个独立的判据，别混在一起：
    //   进程活着 = 机器人起来了；
    //   HTTP 端口在听 = OneBot 的 HTTP 服务真的开了（这才决定推送能不能用）。
    // 用户踩过的 404 就是「进程在跑、但 HTTP 服务端没开」。
    const svc = b.onebot_http_port
      ? `<span class="tag ok">${esc(t('bot.http_on'))} :${b.onebot_http_port}</span>`
      : (b.installed
        ? `<span class="tag" style="color:#d9a343">${esc(t('bot.http_off'))}</span>`
        : `<span class="tag">${esc(t('bot.service_down'))}</span>`);

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

    // 一键配置：只有当装好了、而且 HTTP 服务端确实没开时才显眼地摆出来 ——
    // 已经配好的用户不该被这一块占走注意力
    const needCfg = b.installed && !b.onebot_http_port;
    const cfgBox = !b.installed ? '' : `
      <div class="note" style="display:block;${needCfg ? '' : 'opacity:.7'}">
        <b>${esc(t('bot.cfg_title'))}</b><br>
        ${b.onebot_config
          ? `${esc(t('bot.cfg_file'))}<code>${esc(b.onebot_config)}</code>${
              b.onebot_qq ? ` · QQ ${esc(b.onebot_qq)}` : ''}<br>`
          : `<span style="color:#d9a343">${esc(t('bot.cfg_notfound'))}</span><br>`}
        <span style="display:inline-block;margin-top:8px">
          ${esc(t('bot.cfg_port'))}
          <input type="text" id="cfg-port" value="${b.onebot_http_port || 3000}"
                 style="width:80px;display:inline-block">
          ${esc(t('bot.cfg_token'))}
          <input type="text" id="cfg-token" value="" placeholder="${esc(t('bot.cfg_token_ph'))}"
                 style="width:200px;display:inline-block">
          <button class="btn sm ${needCfg ? 'primary' : ''}" id="btn-cfg">${esc(t('bot.cfg_apply'))}</button>
        </span>
        <br><span style="font-size:12px;color:var(--text-dim2)">${esc(t('bot.cfg_hint'))}</span>
      </div>`;

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
      ${cfgBox}
      <div class="row" style="margin:14px 0 6px">
        <b style="font-size:12.5px">${esc(t('bot.log'))}</b>
        <span class="spacer"></span>
        <label class="check" style="padding-top:0">
          <input type="checkbox" id="log-follow" checked> ${esc(t('bot.log_follow'))}
        </label>
        <button class="btn ghost sm" id="btn-log-refresh">${esc(t('bot.log_refresh'))}</button>
        <button class="btn ghost sm" id="btn-log-clear">${esc(t('bot.log_clear'))}</button>
      </div>
      <pre class="logbox" id="bot-log">${esc(log || t('bot.log_empty'))}</pre>
      <p class="hint" style="margin-top:10px">${esc(t('bot.scan_hint'))}</p>
    </div>`;
  };

  // 日志本地缓存一份：点「清空」只清屏幕、不动磁盘上的日志文件 ——
  // 用户想清的是刷屏的噪音，不是证据。
  let logHidden = false;

  const paintBot = async () => {
    const b = await api('/api/bot/napcat');
    if (!b.ok) { botHost.innerHTML = ''; return; }
    const follow = document.getElementById('log-follow');
    const keepFollow = follow ? follow.checked : true;
    const scrollTop = (() => {
      const el = document.getElementById('bot-log');
      return el ? el.scrollTop : null;
    })();

    botHost.innerHTML = botHtml(b);

    // 装完/配完之后把日志区恢复成「跟着看」的状态
    if (b.installing) setTimeout(paintBot, 2000);
    // 启动之后自动刷几次日志，让用户马上看到扫码信息
    if (b.running && follow) scheduleLogPoll();

    const logEl = document.getElementById('bot-log');
    if (logEl) {
      if (logHidden) logEl.textContent = t('bot.log_cleared');
      if (keepFollow && !logHidden) logEl.scrollTop = logEl.scrollHeight;
      else if (scrollTop !== null) logEl.scrollTop = scrollTop;
    }

    const cfgBtn = document.getElementById('btn-cfg');
    if (cfgBtn) {
      cfgBtn.addEventListener('click', async () => {
        const note = document.getElementById('bot-note');
        note.style.display = 'block';
        note.textContent = t('bot.cfg_applying');
        cfgBtn.disabled = true;
        const port = Number(document.getElementById('cfg-port').value) || 3000;
        const token = document.getElementById('cfg-token').value.trim();
        const r = await api('/api/bot/napcat/configure', { port, token });
        cfgBtn.disabled = false;
        if (!r.ok) {
          note.innerHTML = `<b style="color:#e05c5c">${esc(t('common.save_failed'))}</b><br>`
            + `<code style="white-space:pre-wrap">${esc(r.error || '')}</code>`;
          return;
        }
        note.innerHTML = `<b style="color:#7fd3ba">${esc(r.message || '')}</b><br>`
          + `<code>${esc(r.path || '')}</code>`;
        toast(t('bot.cfg_title'), r.message || '');
        await paintBot();
      });
    }

    const refresh = document.getElementById('btn-log-refresh');
    if (refresh) refresh.addEventListener('click', () => { logHidden = false; paintBot(); });

    const clear = document.getElementById('btn-log-clear');
    if (clear) {
      clear.addEventListener('click', () => {
        logHidden = true;
        const el = document.getElementById('bot-log');
        if (el) el.textContent = t('bot.log_cleared');
      });
    }

    botHost.querySelectorAll('button[data-bot]').forEach(btn =>
      btn.addEventListener('click', async () => {
        const act = btn.dataset.bot;
        if (act === 'opendir') { await api('/api/bot/napcat/open', {}); return; }
        const n = document.getElementById('bot-note');
        n.style.display = 'block';
        n.textContent = act === 'install' ? t('bot.installing') : '…';
        btn.disabled = true;
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

  // 启动后一段时间内勤刷日志（扫码、报错都在这几秒里出来），
  // 之后停下来 —— 常驻轮询只会白烧 CPU，这台机器还要录课。
  let logPolls = 0;
  const scheduleLogPoll = () => {
    if (logPolls >= 10) return;
    logPolls += 1;
    setTimeout(() => { if (current === 'push') paintBot(); }, 2500);
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

  // 当前编辑的时间表。ClassIsland 的编辑窗口是「左侧选、中间编、右侧看详情」，
  // 这里保持同样的心智：选中的那一份才是下面所有操作的作用对象。
  let list = (d.all || []).map(t => ({ id: t.id || 'default', name: t.name || t.id || 'default', slots: (t.slots || []).slice() }));
  if (!list.length) list = [{ id: 'default', name: '', slots: [] }];
  let cur = Math.max(0, list.findIndex(t => t.id === d.id));
  let sel = 0; // 选中的时间点（表格行的下标）

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('st.title'))}</h2>
    <p class="hint">${t('st.hint')}</p>
    <div class="tt-wrap">
      <div class="tt-side">
        <div class="tt-side-hd">${esc(t('st.pick_timetable'))}</div>
        <div id="tt-list"></div>
        <button class="btn sm" id="btn-tt-new" style="margin-top:8px">${esc(t('st.self.new'))}</button>
        <div class="row" style="margin-top:6px">
          <button class="btn sm" id="btn-tt-copy">${esc(t('st.self.copy'))}</button>
          <button class="btn sm danger" id="btn-tt-del">${esc(t('st.self.del'))}</button>
        </div>
        <button class="btn sm" id="btn-ci" style="margin-top:10px">${esc(t('st.import_ci'))}</button>
        <input type="file" id="ci-file" accept=".json,application/json" style="display:none">
      </div>
      <div class="tt-main">
        <input type="text" id="tt-name" style="margin-bottom:10px;font-weight:600">
        <div class="slot-row slot-head">
          <div>${esc(t('st.col_period'))}</div><div>${esc(t('st.col_start'))}</div>
          <div>${esc(t('st.col_end'))}</div><div>${esc(t('st.col_dur'))}</div>
          <div>${esc(t('st.col_kind'))}</div><div>${esc(t('st.col_name'))}</div>
          <div>${esc(t('st.col_no'))}</div>
        </div>
        <div id="slots"></div>
        <div class="row" style="margin-top:10px">
          <button class="btn sm" data-add="class">+ ${esc(t('st.kind_class'))}</button>
          <button class="btn sm" data-add="break">+ ${esc(t('st.kind_break_ci'))}</button>
          <span class="spacer"></span>
          <span style="font-size:12px;color:var(--text-dim2)">${esc(t('st.auto'))}</span>
          <button class="btn primary" id="btn-save">${esc(t('common.save'))}</button>
        </div>
        <p class="hint" style="margin-top:8px">${esc(t('st.default_40'))}</p>
        <div class="tl" id="tl"></div>
        <div class="note" id="win-note" style="display:none"></div>
        <div class="note" id="detail" style="display:block;margin-top:10px"></div>
      </div>
    </div>
  </div>`;

  const host = document.getElementById('slots');
  const nameEl = document.getElementById('tt-name');
  const COLS = '52px 82px 82px 74px 96px 1fr 138px';

  /* ---- 左侧：时间表列表（对应 ClassIsland 左侧那一栏）---- */
  const paintList = () => {
    document.getElementById('tt-list').innerHTML = list.map((x, i) =>
      `<div class="tt-item ${i === cur ? 'active' : ''}" data-i="${i}">${esc(x.name || x.id)}</div>`).join('');
    document.querySelectorAll('#tt-list .tt-item').forEach(el =>
      el.addEventListener('click', () => {
        cur = Number(el.dataset.i);
        sel = 0;
        paintAll();
      }));
    nameEl.value = list[cur].name;
  };

  /* ---- 数字列与时长：时长由起止时间算出来，改一个另一个跟着动 ---- */
  const durOf = (s, e) => {
    const a = /^(\d{1,2}):(\d{2})$/.exec(String(s).trim());
    const b = /^(\d{1,2}):(\d{2})$/.exec(String(e).trim());
    if (!a || !b) return '';
    return String(Number(b[1]) * 60 + Number(b[2]) - (Number(a[1]) * 60 + Number(a[2])));
  };

  const build = (s, i) => el(`<div class="slot-row" data-i="${i}" style="grid-template-columns:${COLS}">
      <div style="color:var(--text-dim2);font-family:var(--mono);font-size:12px;padding-top:8px">0</div>
      <input type="text" data-k="start" value="${esc(s.start || '08:00')}" placeholder="08:00">
      <input type="text" data-k="end" value="${esc(s.end || '08:45')}" placeholder="08:45">
      <input type="text" data-k="duration" value="${esc(durOf(s.start || '08:00', s.end || '08:45'))}">
      <select data-k="kind">
        <option value="class" ${s.kind !== 'break' ? 'selected' : ''}>${esc(t('st.kind_class'))}</option>
        <option value="break" ${s.kind === 'break' ? 'selected' : ''}>${esc(t('st.kind_break_ci'))}</option>
      </select>
      <input type="text" data-k="name" value="${esc(s.name || '')}" placeholder="${esc(t('st.name_ph'))}">
      <div class="row-ops">
        <button class="btn sm" data-op="up" title="${esc(t('st.op_up'))}">↑</button>
        <button class="btn sm" data-op="down" title="${esc(t('st.op_down'))}">↓</button>
        <button class="btn sm" data-op="ins" title="${esc(t('st.op_insert'))}">+</button>
        <button class="btn sm danger" data-op="del" title="${esc(t('st.op_del'))}">×</button>
      </div>
    </div>`);

  const paint = () => {
    const slots = list[cur].slots;
    host.innerHTML = '';
    if (!slots.length) {
      // 空表给一个能直接改的起点，而不是空白 —— 打开就能排
      slots.push({ start: '08:00', end: '08:40', kind: 'class', name: null });
    }
    slots.forEach((s, i) => host.appendChild(build(s, i)));
    renumber();
    paintTimeline();
    paintDetail();
  };

  const renumber = () => {
    const rows = [...host.children];
    let p = 0;
    rows.forEach((r, i) => {
      const kind = r.querySelector('[data-k="kind"]').value;
      p = kind === 'class' ? p + 1 : p;
      r.firstElementChild.textContent = kind === 'class' ? p : '—';
      r.classList.toggle('sel', i === sel);
    });
  };

  /* ---- 时间轴预览：每一段按分钟数占宽度，一眼看出一天的松紧 ----
     对齐 ClassIsland 时间轴视图：拖动段的两端改起止时间，拖中间平移整段。
     段本身很窄（只有几分钟宽），靠把手抓不准，所以按鼠标在段内的相对位置判断：
     左 22% 改开始、右 22% 改结束、中间整段平移。 */
  const paintTimeline = () => {
    const slots = list[cur].slots;
    document.getElementById('tl').innerHTML =
      `<div class="tl-hd">${esc(t('st.timeline'))}</div>` +
      `<div class="tl-hint">${esc(t('st.tl_hint'))}</div><div class="tl-bar">` +
      slots.map((s, i) => {
        const m = Math.max(0, Number(durOf(s.start, s.end)) || 0);
        const cls = s.kind === 'break' ? 'brk' : 'cls';
        return `<div class="tl-seg ${cls} ${i === sel ? 'sel' : ''}" data-i="${i}"
          style="flex:${m}" title="${esc(s.start)}–${esc(s.end)} ${esc(t('st.dur_min').replace('{n}', m))}">${m >= 20 ? m : ''}</div>`;
      }).join('') + '</div>';
    document.querySelectorAll('#tl .tl-seg').forEach(el =>
      el.addEventListener('click', () => { sel = Number(el.dataset.i); renumber(); paintDetail(); paintTimelineOnly(); }));
  };

  /* ---- 时间轴上的拖动编辑（对齐 ClassIsland 拖把柄改时间）----
     按下时记下起点，移动超过 3px 才算拖动（否则当点击选中）。
     像素 → 分钟按「这一段的宽度 = 它的分钟数」换算，段越窄越精细，至少 5 分钟一档。

     绑定挂在 `#tl` 这个**静态**节点上（它整个页面生命周期里只创建一次），
     用事件委托找 `.tl-seg`。这样时间轴每次重绘都不需要重新绑定 ——
     之前把 onmousedown 挂在 `.tl-bar` 上，而 `.tl-bar` 每次重绘都会被换掉，
     于是重绘之后就再也点不动了（实测踩过：第一次能拖，之后拖不动）。 */
  const tlDrag = { on: false };
  document.getElementById('tl').onmousedown = (ev) => {
    const el = ev.target.closest('.tl-seg');
    if (!el) return;
    const i = Number(el.dataset.i);
    const s = list[cur].slots[i];
    if (!s) return;
    const r = el.getBoundingClientRect();
    const x = (ev.clientX - r.left) / Math.max(1, r.width);
    // 段本身可能只有几分钟宽，靠把手抓不准，所以按鼠标在段内的相对位置定模式
    Object.assign(tlDrag, {
      on: true, i, el, moved: false, x0: ev.clientX,
      mode: x < 0.22 ? 'start' : x > 0.78 ? 'end' : 'move',
      start: s.start, end: s.end,
    });
    sel = i;
    ev.preventDefault();
  };

  window.addEventListener('mousemove', (ev) => {
    if (!tlDrag.on) return;
    syncTimelineDrag(ev.clientX);
  });
  window.addEventListener('mouseup', () => {
    if (!tlDrag.on) return;
    const moved = tlDrag.moved;
    tlDrag.on = false;
    if (moved) { renumber(); paintDetail(); paintTimeline(); }
  });

  /** 按当前鼠标位置更新正在拖的那一段。 */
  const syncTimelineDrag = (clientX) => {
    const dx = clientX - tlDrag.x0;
    if (!tlDrag.moved && Math.abs(dx) < 3) return;
    tlDrag.moved = true;
    const whole = Number(durOf(tlDrag.start, tlDrag.end)) || 1;
    const perMin = Math.max(1, tlDrag.el.getBoundingClientRect().width) / whole;
    // 至少 5 分钟一档，否则手一抖就变成 1 分钟
    const delta = Math.trunc(dx / perMin / 5) * 5;
    const s = list[cur].slots[tlDrag.i];
    if (!s) return;
    if (tlDrag.mode === 'start') {
      const t2 = plusMin(tlDrag.start, Math.min(delta, whole - 5));
      s.start = t2;
      s.end = plusMin(t2, whole);
    } else if (tlDrag.mode === 'end') {
      s.start = tlDrag.start;
      s.end = plusMin(tlDrag.end, Math.max(delta, -(whole - 5)));
    } else {
      s.start = plusMin(tlDrag.start, delta);
      s.end = plusMin(tlDrag.end, delta);
    }
    const row = host.children[tlDrag.i];
    if (row) {
      row.querySelector('[data-k="start"]').value = s.start;
      row.querySelector('[data-k="end"]').value = s.end;
      row.querySelector('[data-k="duration"]').value = durOf(s.start, s.end);
    }
    paintTimelineOnly();
  };

  /** 拖动过程中只重绘时间轴（重建整张表会把正在拖的元素换掉，拖动就断了）。 */
  const paintTimelineOnly = () => {
    const slots = list[cur].slots;
    const segs = document.querySelectorAll('#tl .tl-seg');
    slots.forEach((s, i) => {
      const el = segs[i];
      if (!el) return;
      const m = Math.max(0, Number(durOf(s.start, s.end)) || 0);
      el.style.flex = String(m);
      el.title = `${s.start}–${s.end} ${t('st.dur_min').replace('{n}', String(m))}`;
      el.textContent = m >= 20 ? String(m) : '';
      el.classList.toggle('sel', i === sel);
    });
  };

  /* ---- 右侧详情：选中时间点的详细属性（对应 ClassIsland 视图右侧）---- */
  const paintDetail = () => {
    const box = document.getElementById('detail');
    const s = list[cur].slots[sel];
    if (!s) { box.innerHTML = esc(t('st.detail_none')); return; }
    box.innerHTML = `<b>${esc(t('st.detail'))}</b>` +
      `<div class="row wrap" style="margin-top:8px;align-items:center;gap:10px">
        <label style="font-size:13px">${esc(t('st.period_default'))}
          <input type="text" id="dt-default" value="${esc(s.default_subject || '')}"
                 placeholder="${esc(t('st.period_default_ph'))}" style="width:190px;margin-left:6px"></label>
        <label class="check" title="${esc(t('st.hidden_hint'))}">
          <input type="checkbox" id="dt-hidden" ${s.is_hidden ? 'checked' : ''}> ${esc(t('st.hidden'))}</label>
      </div>`;
    document.getElementById('dt-default').addEventListener('input', e => {
      list[cur].slots[sel].default_subject = e.target.value.trim() || null;
    });
    document.getElementById('dt-hidden').addEventListener('change', e => {
      list[cur].slots[sel].is_hidden = e.target.checked;
    });
  };

  /* ---- 表格编辑：开始 / 结束 / 时长三者联动 ---- */
  const rowOf = (node) => node.closest('.slot-row');
  const vOf = (row, k) => row.querySelector(`[data-k="${k}"]`);

  host.addEventListener('input', (e) => {
    const row = rowOf(e.target);
    if (!row) return;
    const i = [...host.children].indexOf(row);
    const k = e.target.dataset.k;
    if (!k) return;
    const s = list[cur].slots[i];
    s[k] = e.target.value.trim() || (k === 'kind' ? 'class' : null);
    if (k === 'start') {
      // 开始时间一改，结束时间顺延（时长保持不变）—— 与 ClassIsland 拖把柄的手感一致
      const d = Number(vOf(row, 'duration').value) || 0;
      const end = plusMin(e.target.value, d);
      s.end = end;
      vOf(row, 'end').value = end;
    } else if (k === 'end') {
      vOf(row, 'duration').value = durOf(s.start, e.target.value);
    } else if (k === 'duration') {
      const end = plusMin(s.start, Number(e.target.value) || 0);
      s.end = end;
      vOf(row, 'end').value = end;
    }
    renumber();
    // 只更新段的宽度，不重建整条时间轴：重建会连带把绑定丢掉，
    // 而这里每次改时长都会触发
    paintTimelineOnly();
  });

  host.addEventListener('change', (e) => {
    const row = rowOf(e.target);
    if (!row || !e.target.dataset.k) return;
    const i = [...host.children].indexOf(row);
    list[cur].slots[i][e.target.dataset.k] = e.target.value;
    renumber();
  });

  host.addEventListener('click', (e) => {
    const row = rowOf(e.target);
    if (!row) return;
    const i = [...host.children].indexOf(row);
    const btn = e.target.closest('button[data-op]');
    if (!btn) { sel = i; renumber(); paintDetail(); paintTimelineOnly(); return; }
    const slots = list[cur].slots;
    const op = btn.dataset.op;
    if (op === 'del') {
      slots.splice(i, 1);
    } else if (op === 'up' && i > 0) {
      [slots[i - 1], slots[i]] = [slots[i], slots[i - 1]];
      sel = i - 1;
    } else if (op === 'down' && i < slots.length - 1) {
      [slots[i + 1], slots[i]] = [slots[i], slots[i + 1]];
      sel = i + 1;
    } else if (op === 'ins') {
      // 新段接上一段的结束时间；上课 40 分钟、课间 10 分钟 —— 与 ClassIsland 的默认值一致
      const kind = slots[i].kind;
      const len = kind === 'break' ? 10 : 40;
      slots.splice(i + 1, 0, {
        start: slots[i].end, end: plusMin(slots[i].end, len),
        kind, name: null, default_subject: null, is_hidden: false,
      });
      sel = i + 1;
    } else {
      return;
    }
    paint();
  });

  document.querySelectorAll('[data-add]').forEach(b => b.addEventListener('click', () => {
    const kind = b.dataset.add;
    const slots = list[cur].slots;
    const last = slots[slots.length - 1];
    const start = last ? last.end : '08:00';
    slots.push({
      start, end: plusMin(start, kind === 'break' ? 10 : 40),
      kind, name: null, default_subject: null, is_hidden: false,
    });
    sel = slots.length - 1;
    paint();
  }));

  nameEl.addEventListener('input', () => { list[cur].name = nameEl.value; paintList(); });

  const paintAll = () => { paintList(); paint(); };

  /* ---- 新建 / 复制 / 删除 ---- */
  document.getElementById('btn-tt-new').addEventListener('click', () => {
    list.push({ id: 'tl' + (list.length + 1) + '_' + Math.random().toString(36).slice(2, 6),
      name: t('st.self.default_name'), slots: [] });
    cur = list.length - 1;
    sel = 0;
    paintAll();
  });

  document.getElementById('btn-tt-copy').addEventListener('click', () => {
    const src = list[cur];
    list.push({ id: 'tl' + (list.length + 1) + '_' + Math.random().toString(36).slice(2, 6),
      name: src.name + t('st.self.copy_suffix'), slots: src.slots.map(s => ({ ...s })) });
    cur = list.length - 1;
    sel = 0;
    paintAll();
  });

  document.getElementById('btn-tt-del').addEventListener('click', async () => {
    // 删除保护：有课表引用它就不许删 —— 否则那份课表会变成一堆没有时间的空条目。
    // ClassIsland 的规则也是「时间表必须没有被任何课表使用」。
    const sc = await api('/api/schedule');
    const inUse = sc.ok && sc.time_layout_id === list[cur].id;
    if (inUse) { toast(t('st.self.del'), t('st.self.in_use'), 'err'); return; }
    if (!confirm(t('st.self.delete_confirm').replace('{name}', list[cur].name))) return;
    list.splice(cur, 1);
    if (!list.length) list = [{ id: 'default', name: '', slots: [] }];
    cur = 0;
    sel = 0;
    paintAll();
  });

  /* ---- 保存：整份时间表列表一起提交 ---- */
  document.getElementById('btn-save').addEventListener('click', async () => {
    const payload = {
      timetables: list.map((x, i) => ({
        id: x.id, name: x.name || x.id, is_active: i === cur,
        source: 'manual', group: 'global',
        slots: (() => {
          // 节次只在「上课」段之间连续编号：课间不算一节，
          // 否则第 1 节后面直接跳到第 3 节，看着像缺了一节。
          let p = 0;
          return x.slots.map(s => {
            const isClass = s.kind !== 'break';
            if (isClass) p++;
            return {
              period: isClass ? p : null,
              start: String(s.start || '').trim(),
              end: String(s.end || '').trim(),
              kind: isClass ? 'class' : 'break',
              name: s.name || null,
              default_subject: s.default_subject || null,
              is_hidden: !!s.is_hidden,
            };
          });
        })(),
      })),
    };
    const r = await api('/api/timetable', payload);
    const note = document.getElementById('win-note');
    note.style.display = 'block';
    if (r.ok) {
      toast(t('common.saved'), String(r.slots));
      note.innerHTML = (r.issues && r.issues.length)
        ? `<b style="color:#d9a343">${esc(t('sc.issues'))}</b><br>· ` + r.issues.map(esc).join('<br>· ')
        : `<b style="color:#7fd3ba">${esc(t('st.windows'))}</b>` +
          (r.windows || []).map(x => `<br>· ${esc(x.name)}（${esc(x.start)}–${esc(x.end)}，scope=${esc(x.scope)}）`).join('');
    } else {
      toast(t('common.save_failed'), r.error || '', 'err');
      note.innerHTML = `<b style="color:#e05c5c">${esc(t('common.save_failed'))}</b><br><code>${esc(r.error || '')}</code>`;
    }
  });

  /* ---- ClassIsland 导入 ---- */
  // 浏览器拿不到文件真实路径（安全限制），只能读内容，所以整份 JSON 传给后端解析。
  // 后端那边是只读的，不会改 ClassIsland 的任何文件。
  document.getElementById('btn-ci').addEventListener('click', () => document.getElementById('ci-file').click());
  document.getElementById('ci-file').addEventListener('change', async (e) => {
    const f = e.target.files && e.target.files[0];
    if (!f) return;
    const text = await f.text();
    const r = await api('/api/import/classisland', { json: text });
    e.target.value = '';
    if (!r.ok) { toast(t('common.save_failed'), r.error || '', 'err'); return; }
    toast(t('st.imported'), `${r.timetable_name || ''} · ${r.slots} / ${r.entries}`);
    await renderTimetable();
    const n2 = document.getElementById('win-note');
    if (n2) {
      n2.style.display = 'block';
      n2.innerHTML = `<b>${esc(t('st.imported'))}</b> · ${esc(r.timetable_name || '')} ` +
        `<span style="color:var(--text-dim2)">${r.slots} / ${r.entries}</span><br>${esc(t('st.import_hint'))}`;
    }
    setTimeout(() => go('schedule'), 1200);
  });

  paintAll();
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

  // 上课时间点清单：课表的列头就是它 —— 这正是 ClassIsland 的做法
  // （课程格按时间点索引对齐，而不是各自记一份起止时间）。
  const slots = d.class_slots || [];
  const layouts = d.time_layouts || [];

  // 唯一的真数据源。按「星期 + 第几个时间点」索引，网格视图与列表视图
  // 都只改它，所以切视图天然互通，也不需要「切模式时重新读盘」这种补丁。
  let model = {};
  const cell = (day, i) => (model[day] = model[day] || {})[i] ||
    ((model[day][i] = { subject: '', teacher: '', record: true, cycle: 'every' }));

  (d.entries || []).forEach(e => {
    if (!e.day) return;
    // 旧数据可能没有节次，只能按开始时间找它落在哪个时间点上
    let i = (e.period || 0) - 1;
    if (i < 0 || i >= slots.length || slots[i].start !== e.start) {
      const hit = slots.findIndex(s => s.start === e.start);
      if (hit >= 0) i = hit;
    }
    if (i < 0) i = 0;
    const c = cell(e.day, i);
    c.subject = e.course || '';
    c.teacher = nameOf(e.teacherId);
    c.record = e.record !== false;
    c.cycle = e.cycle || 'every';
  });

  let mode = 'grid';
  const rule = d.time_rule || {};
  let ruleWeekday = rule.weekday || 0;
  let ruleDiv = (rule.week_count && rule.week_count.week) || 0;
  let layoutId = d.time_layout_id || (layouts[0] && layouts[0].id) || '';
  let enabled = d.is_enabled !== false;

  const secOpts = (v) => [
    [0, t('sc.rule_daily')], [1, t('sc.day_mon')], [2, t('sc.day_tue')], [3, t('sc.day_wed')],
    [4, t('sc.day_thu')], [5, t('sc.day_fri')], [6, t('sc.day_sat')], [7, t('sc.day_sun')],
  ].map(([k, s]) => `<option value="${k}" ${Number(v) === k ? 'selected' : ''}>${esc(s)}</option>`).join('');

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('sc.title'))}</h2>
    <p class="hint">${t('sc.hint')}</p>

    <div class="row wrap" style="margin-bottom:12px;align-items:center;gap:10px">
      <label style="font-size:13px">${esc(t('sc.bind_layout'))}
        <select id="bind" style="margin-left:6px">
          ${layouts.map(t2 => `<option value="${esc(t2.id)}" ${t2.id === layoutId ? 'selected' : ''}>${esc(t2.name || t2.id)}</option>`).join('') ||
            `<option value="">${esc(t('sc.no_layout'))}</option>`}
        </select></label>
      <label style="font-size:13px">${esc(t('sc.rule_weekday'))}
        <select id="rule-day" style="margin-left:6px">${secOpts(ruleWeekday)}</select></label>
      <label style="font-size:13px">${esc(t('sc.rule_cycle'))}
        <select id="rule-div" style="margin-left:6px">
          <option value="0" ${ruleDiv === 0 ? 'selected' : ''}>${esc(t('sc.rule_none'))}</option>
          <option value="1" ${ruleDiv === 1 ? 'selected' : ''}>${esc(t('sc.rule_odd'))}</option>
          <option value="2" ${ruleDiv === 2 ? 'selected' : ''}>${esc(t('sc.rule_even'))}</option>
        </select></label>
      <label class="check"><input type="checkbox" id="rule-on" ${enabled ? 'checked' : ''}> ${esc(t('sc.rule_enabled'))}</label>
    </div>

    <div class="row wrap" style="margin-bottom:12px">
      <div class="seg" id="mode">
        <button class="seg-btn active" data-mode="grid">${esc(t('sc.mode_grid'))}</button>
        <button class="seg-btn" data-mode="table">${esc(t('sc.mode_table'))}</button>
      </div>
      <span class="spacer"></span>
      <span style="font-size:12px;color:var(--text-dim2)" id="cnt"></span>
      <button class="btn sm" id="btn-csv">${esc(t('sc.export'))}</button>
      <button class="btn primary" id="btn-save">${esc(t('common.save'))}</button>
    </div>

    <!-- 录播批量勾选。
         原先只有两个"全部开启/全部关闭"，而且它们作用于**全部格子**（含空格），
         想只勾其中几节课只能一格格点。这里换成真正的批量选择：
         全选 / 取消全选 / 反选，作用域是**当前筛选结果**（见 bulkScopeNote）。 -->
    <div class="bulk-bar">
      <input type="text" id="sc-filter" class="bulk-search" placeholder="${esc(t('sc.filter_ph'))}">
      <button class="btn sm" id="btn-clear-filter" style="display:none">${esc(t('sc.filter_clear'))}</button>
      <div class="bulk-ops">
        <button class="btn sm" id="btn-on">${esc(t('sc.bulk_all'))}<span class="bulk-scope" id="scope-on"></span></button>
        <button class="btn sm" id="btn-off">${esc(t('sc.bulk_none'))}</button>
        <button class="btn sm" id="btn-invert">${esc(t('sc.bulk_invert'))}<span class="bulk-scope" id="scope-inv"></span></button>
      </div>
    </div>
    <p class="hint" id="bulk-note" style="margin:0 0 10px"></p>

    <div id="body"></div>
    <div class="note" id="save-note" style="display:none"></div>
  </div>`;

  const body = document.getElementById('body');

  // 搜索关键词。空 = 不过滤。
  let keyword = '';

  /**
   * 参与批量选择与计数的条目。
   *
   * # 为什么必须和网格里的格子用同一套判定
   *
   * 网格是「星期 × 节次」的**全量**格子，绝大部分是空的。批量勾选如果
   * 把空格也算进去，"全选"看着勾了 200 个、实际只有 12 节课，计数就假了。
   * 所以统一定义：**填了科目或教师**的格子才算"一门课"。
   *
   * 返回 [{ day, i, c }]，顺序与列表视图一致（星期 → 节次），
   * 这样"反选"之后用户在两个视图里看到的顺序是同一个。
   */
  const items = () => {
    const out = [];
    Object.keys(model).forEach(day => Object.keys(model[day]).forEach(i => {
      const c = model[day][i];
      if ((c.subject || '').trim() || (c.teacher || '').trim()) {
        out.push({ day, i: Number(i), c });
      }
    }));
    const order = DAYS.map(d => d[0]);
    out.sort((a, b) => order.indexOf(a.day) - order.indexOf(b.day) || a.i - b.i);
    return out;
  };

  /** 关键词命中的条目（全选/反选/计数都只看它）。 */
  const visibleItems = () => {
    const all = items();
    const kw = keyword.trim().toLowerCase();
    if (!kw) return all;
    return all.filter(it =>
      (it.c.subject || '').toLowerCase().includes(kw) ||
      (it.c.teacher || '').toLowerCase().includes(kw));
  };

  /**
   * 刷新计数、作用域提示与空状态。
   *
   * 三个数字各说各话最容易出问题，所以在这里一次算清：
   * - `m` 总课程数（填了内容的格子）
   * - `n` 已勾选数（**不受筛选影响** —— 筛掉不等于取消勾选，
   *       否则用户搜一下再清空搜索，勾选就没了，这属于静默丢数据）
   * - `v` 当前可见数（筛选命中）
   */
  const upd = () => {
    const all = items();
    const vis = visibleItems();
    const m = all.length;
    const n = all.filter(it => it.c.record).length;
    const filtered = keyword.trim().length > 0;

    const cnt = document.getElementById('cnt');
    if (cnt) {
      if (!m) {
        cnt.textContent = t('sc.count_none');
      } else if (filtered) {
        // 有筛选时把"筛选范围内勾了几个"也说清楚。
        //
        // 只报全局的 n/m 会造成困惑：列表被筛得只剩 1 行，计数却说"已选 4"，
        // 用户会以为勾选没生效。所以补一个 v 维度：
        // 「已选 4 / 共 12 个 · 当前 1 项中 1 项已选」。
        // 两个数都在，谁也误导不了谁。
        const vOn = vis.filter(it => it.c.record).length;
        cnt.textContent = t('sc.count_selected')
          .replace('{n}', String(n)).replace('{m}', String(m))
          + ' · ' + t('sc.count_in_filter')
            .replace('{n}', String(vOn)).replace('{m}', String(vis.length));
      } else {
        cnt.textContent = t('sc.count_selected')
          .replace('{n}', String(n)).replace('{m}', String(m));
      }
    }

    // 作用域提示：全选到底会勾多少，直接标在按钮上，避免误解
    const so = document.getElementById('scope-on');
    const si = document.getElementById('scope-inv');
    if (so) so.textContent = filtered ? t('sc.bulk_scope_filtered').replace('{n}', String(vis.length)) : '';
    if (si) si.textContent = filtered ? t('sc.bulk_scope_filtered').replace('{n}', String(vis.length)) : '';

    const note = document.getElementById('bulk-note');
    if (note) {
      // 只在使用者需要知道的时候说话：一门课都没有时给出引导，
      // 其余情况交给按钮上的作用域标注与计数，不再重复
      note.textContent = m ? '' : t('sc.empty_no_course');
    }

    // 清除按钮只在有内容时出现，免得多个常年无用的按钮
    const cf = document.getElementById('btn-clear-filter');
    if (cf) cf.style.display = keyword ? '' : 'none';

    document.querySelectorAll('#mode .seg-btn').forEach(b =>
      b.classList.toggle('active', b.dataset.mode === mode));
  };

  /** 批量改录播开关。`pick(item, idx) -> bool` 决定每个条目勾不勾。 */
  const bulkSet = (pick) => {
    visibleItems().forEach((it, idx) => { it.c.record = pick(it, idx); });
    paint();
  };

  /* ---- 科目 / 教师候选：与 ClassIsland 的「科目」库对应 ---- */
  const subjects = () => {
    const s = new Set();
    Object.keys(model).forEach(day => Object.keys(model[day]).forEach(i => {
      const v = (model[day][i].subject || '').trim();
      if (v) s.add(v);
    }));
    return [...s].sort();
  };
  const teachers = () => {
    const s = new Set();
    Object.keys(model).forEach(day => Object.keys(model[day]).forEach(i => {
      const v = (model[day][i].teacher || '').trim();
      if (v) s.add(v);
    }));
    return [...s].sort();
  };
  const dl = () => `<datalist id="dl-course">${subjects().map(s => `<option value="${esc(s)}">`).join('')}</datalist>
    <datalist id="dl-teacher">${teachers().map(s => `<option value="${esc(s)}">`).join('')}</datalist>`;

  /* ---- 模式一：课程格。列 = 时间点，行 = 星期（对齐 ClassIsland 的课表编辑）---- */
  const paintGrid = () => {
    if (!slots.length) {
      body.innerHTML = `<div class="empty">${esc(t('sc.no_layout_why'))}</div>`;
      return;
    }
    // 筛选在网格里只做**变暗**，不做隐藏。
    // 网格是编辑用的画布，藏掉格子会让用户以为课没了、
    // 也可能在看不见的地方改错行。一眼看出"哪些不在筛选范围里"就够了。
    const kw = keyword.trim().toLowerCase();
    const dimmed = (c) => {
      if (!kw) return false;
      if (!(c.subject || '').trim() && !(c.teacher || '').trim()) return false;
      return !((c.subject || '').toLowerCase().includes(kw) ||
        (c.teacher || '').toLowerCase().includes(kw));
    };
    body.innerHTML = dl() + `<div class="sc-scroll"><table class="sc-grid"><thead><tr>
      <th class="sc-day"></th>
      ${slots.map(s => `<th><div class="sc-th-t">${esc(s.start)}</div>
        <div class="sc-th-s">${esc(t('sc.period_n').replace('{n}', String((s.period || 0))))}${s.duration ? ` · ${esc(t('st.dur_min').replace('{n}', String(s.duration)))}` : ''}</div></th>`).join('')}
      </tr></thead><tbody>
      ${DAYS.map(([day, key]) => `<tr>
        <td class="sc-day">${esc(t('sc.' + key))}</td>
        ${slots.map((s, i) => {
          const c = cell(day, i);
          const ph = esc(s.default_subject || t('sc.cell_ph'));
          const dim = dimmed(c) ? ' sc-dim' : '';
          return `<td><div class="sc-cell${dim}">
            <input type="text" data-day="${day}" data-i="${i}" data-k="subject"
                   list="dl-course" value="${esc(c.subject)}" placeholder="${ph}">
            <div class="sc-sub">
              <input type="text" data-day="${day}" data-i="${i}" data-k="teacher"
                     list="dl-teacher" value="${esc(c.teacher)}" placeholder="${esc(t('sc.col_teacher'))}">
              <label class="check" title="${esc(t('sc.record_hint'))}">
                <input type="checkbox" data-day="${day}" data-i="${i}" data-k="record" ${c.record ? 'checked' : ''}></label>
            </div>
          </div></td>`;
        }).join('')}
      </tr>`).join('')}
      </tbody></table></div>`;
    body.querySelectorAll('input[data-k]').forEach(node => {
      const c = () => cell(node.dataset.day, Number(node.dataset.i));
      if (node.dataset.k === 'record') {
        node.addEventListener('change', () => { c().record = node.checked; upd(); });
      } else {
        node.addEventListener('input', () => { c()[node.dataset.k] = node.value; });
        // 科目/教师改了会影响筛选结果与计数，所以 change 时重绘 ——
        // 只 input 时重绘会把正在输入的光标位置弄丢
        node.addEventListener('change', () => { c()[node.dataset.k] = node.value; paint(); });
      }
    });
  };

  /* ---- 模式二：逐条编辑。同一条数据换个角度看，方便批量核对 ---- */
  const COLS = '92px 74px 74px 1fr 120px 68px 58px 136px';
  const flat = () => {
    const rows = [];
    Object.keys(model).forEach(day => Object.keys(model[day]).forEach(i => {
      const c = model[day][i];
      if (c.subject.trim() || c.teacher.trim()) rows.push({ day, i: Number(i), c });
    }));
    rows.sort((a, b) => DAYS.findIndex(x => x[0] === a.day) - DAYS.findIndex(x => x[0] === b.day) || a.i - b.i);
    return rows;
  };
  const paintTable = () => {
    // 列表视图是"核对与批量操作"用的，所以这里**真的过滤**（隐藏不匹配的行）——
    // 这和网格视图的变暗策略不同，是有意的：
    // 列表本来就是全量条目的投影，隐藏不匹配的行正好让用户看清"全选会勾哪些"。
    const all = flat();
    const rows = keyword.trim() ? visibleItems() : all;

    // 空状态分三种，不能都显示成空白列表：
    // 课表本来就空 / 筛选没命中 / 正常有内容。
    if (!all.length) {
      body.innerHTML = dl() + `<div class="empty">${esc(t('sc.empty_no_course'))}</div>`;
      return;
    }
    if (!rows.length) {
      body.innerHTML = dl() + `<div class="empty">${esc(t('sc.empty_filtered'))}</div>`;
      return;
    }

    body.innerHTML = dl() + `
      <div class="slot-row slot-head" style="grid-template-columns:${COLS}">
        <div>${esc(t('sc.col_day'))}</div><div>${esc(t('sc.col_start'))}</div>
        <div>${esc(t('sc.col_end'))}</div><div>${esc(t('sc.col_course'))}</div>
        <div>${esc(t('sc.col_teacher'))}</div><div>${esc(t('sc.col_cycle'))}</div>
        <div>${esc(t('sc.col_record'))}</div><div></div>
      </div>` +
      rows.map((r, n) => {
        const s = slots[r.i] || {};
        return `<div class="slot-row" data-n="${n}" style="grid-template-columns:${COLS}">
          <select data-k="day">${DAYS.map(([v, k]) =>
            `<option value="${v}" ${r.day === v ? 'selected' : ''}>${esc(t('sc.' + k))}</option>`).join('')}</select>
          <input type="text" value="${esc(s.start || '')}" disabled>
          <input type="text" value="${esc(s.end || '')}" disabled>
          <input type="text" data-k="subject" list="dl-course" value="${esc(r.c.subject)}" placeholder="${esc(t('sc.course_ph'))}">
          <input type="text" data-k="teacher" list="dl-teacher" value="${esc(r.c.teacher)}" placeholder="${esc(t('sc.teacher_unset'))}">
          <select data-k="cycle" style="width:74px">${CYCLES.map(([v, k]) =>
            `<option value="${v}" ${(r.c.cycle || 'every') === v ? 'selected' : ''}>${esc(t('sc.' + k))}</option>`).join('')}</select>
          <label class="check" title="${esc(t('sc.record_hint'))}">
            <input type="checkbox" data-k="record" ${r.c.record ? 'checked' : ''}></label>
          <div class="row-ops"><button class="btn sm danger" data-op="del" title="${esc(t('st.op_del'))}">×</button></div>
        </div>`;
      }).join('') + `<p class="hint" style="margin-top:10px">${esc(t('sc.table_hint'))}</p>`;

    body.querySelectorAll('.slot-row[data-n]').forEach(row => {
      const r = rows[Number(row.dataset.n)];
      row.querySelectorAll('[data-k]').forEach(node => {
        const k = node.dataset.k;
        if (k === 'day') {
          node.addEventListener('change', () => {
            // 换星期 = 把这一格挪到另一行，原来那格清空
            model[r.day][r.i] = { subject: '', teacher: '', record: true, cycle: 'every' };
            Object.assign(cell(node.value, r.i), r.c);
            paint();
          });
          return;
        }
        if (k === 'record') {
          node.addEventListener('change', () => { r.c.record = node.checked; upd(); });
        } else {
          node.addEventListener('input', () => { r.c[k] = node.value; });
          // 改课名/教师会影响筛选命中与计数，所以 change 时整体重绘
          node.addEventListener('change', () => { r.c[k] = node.value; paint(); });
        }
      });
      row.querySelector('[data-op="del"]').addEventListener('click', () => {
        model[r.day][r.i] = { subject: '', teacher: '', record: true, cycle: 'every' };
        paint();
      });
    });
  };

  const paint = () => { if (mode === 'grid') paintGrid(); else paintTable(); upd(); };

  document.querySelectorAll('#mode .seg-btn').forEach(b =>
    b.addEventListener('click', () => { mode = b.dataset.mode; paint(); }));

  // 批量操作。三者的作用域都是**当前筛选结果**（无筛选时即全部课程），
  // 这一点已经标在按钮上的「（当前 N 项）」里。
  document.getElementById('btn-on').addEventListener('click', () => {
    // 全选：把当前可见的全部勾上（已在勾选中的保持勾选，是幂等的）
    bulkSet(() => true);
  });
  document.getElementById('btn-off').addEventListener('click', () => {
    bulkSet(() => false);
  });
  document.getElementById('btn-invert').addEventListener('click', () => {
    // 反选：按"操作前"的值取反。
    // 注意 pick 是在同一个循环里被逐个调用的，每次读的都是当前值 ——
    // 但每个条目的值只被读一次、写一次，所以不会出现"改了 A 影响 B"。
    bulkSet(it => !it.c.record);
  });

  // 搜索框：输入即过滤（本地过滤，不发请求 —— 课表本来就在内存里）
  const filterEl = document.getElementById('sc-filter');
  filterEl.addEventListener('input', () => {
    keyword = filterEl.value;
    paint();
  });
  document.getElementById('btn-clear-filter').addEventListener('click', () => {
    filterEl.value = '';
    keyword = '';
    paint();
  });

  document.getElementById('bind').addEventListener('change', e => { layoutId = e.target.value; });
  document.getElementById('rule-day').addEventListener('change', e => { ruleWeekday = Number(e.target.value); });
  document.getElementById('rule-div').addEventListener('change', e => { ruleDiv = Number(e.target.value); });
  document.getElementById('rule-on').addEventListener('change', e => { enabled = e.target.checked; });

  document.getElementById('btn-csv').addEventListener('click', () => {
    const rows = [['day', 'period', 'start', 'end', 'course', 'teacher', 'cycle', 'record'].join(',')];
    flat().forEach(r => {
      const s = slots[r.i] || {};
      rows.push([r.day, (s.period || '') + '', s.start || '', s.end || '', r.c.subject, r.c.teacher, r.c.cycle, r.c.record]
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
    const note = document.getElementById('save-note');

    // 空选择拦截。
    //
    // 判的是「有课程、但一节课都没勾」—— 那多半是手滑把全选取消了，
    // 保存下去这一学期就一节课都不录，而界面上不会有任何异常迹象。
    // 课表本来就空的（一门课都没填）不算，那是还没开始编辑，轮不到这里拦。
    const all = items();
    if (all.length && !all.some(it => it.c.record)) {
      note.style.display = 'block';
      note.innerHTML = `<b style="color:#d9a343">${esc(t('sc.select_all_first'))}</b><br>` +
        esc(t('sc.need_pick'));
      toast(t('common.save_failed'), t('sc.need_pick'), 'err');
      return;
    }

    // 姓名 -> id：沿用已有教师；输入了新名字就当场建一个。
    // 名字留空 = 未指定（unassigned），校验那边会提醒，但**不拦保存**。
    const tlist = (d.teachers || []).map(x => ({ ...x }));
    const idOf = (name) => {
      const nm = String(name || '').trim();
      if (!nm) return 'unassigned';
      const hit = tlist.find(x => x.name === nm);
      if (hit) return hit.id;
      let k = tlist.length + 1;
      while (tlist.some(x => x.id === 't' + k)) k++;
      const id = 't' + k;
      tlist.push({ id, name: nm, profile: id });
      return id;
    };

    // 课程格 -> 条目：起止时间不写进课表了 —— 它由绑定的时间表算出来，
    // 这正是 ClassIsland 的做法，时间表一改，所有课的时间自动跟着改。
    const entries = [];
    Object.keys(model).forEach(day => Object.keys(model[day]).forEach(i => {
      const c = model[day][i];
      const s = slots[Number(i)];
      if (!s || (!c.subject.trim() && !c.teacher.trim())) return;
      entries.push({
        day,
        period: Number(i) + 1,
        start: s.start,
        end: s.end,
        course: c.subject.trim() || t('sc.unnamed'),
        teacherId: idOf(c.teacher),
        record: !!c.record,
        cycle: c.cycle || 'every',
      });
    }));

    const r = await api('/api/schedule', {
      teachers: tlist,
      week_template: {
        cycle: 'every',
        entries,
        time_layout_id: layoutId,
        time_rule: { weekday: ruleWeekday, week_count: { week: ruleDiv, total: ruleDiv ? 2 : 0 } },
        is_enabled: enabled,
      },
      weekend_template: { source: 'new', entries: [] },
      overrides: [],
    });
    note.style.display = 'block';
    if (r.ok) {
      toast(t('common.saved'), String(r.entries));
      // 保存会重建教师名单（新名字会被分配 id），回读一遍让界面与磁盘一致
      const back = await api('/api/schedule');
      if (back.ok) { state.schedule = back; }
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

/* ---------------------------------------------------------------- 日志 */

/**
 * 日志页：程序全部输出的集中展示。
 *
 * 背景：这个程序以前会弹一个控制台窗口，所有 println / tracing 输出都在那儿。
 * 那个黑框已经去掉了（GUI 模式下启动即隐藏），于是这些输出必须有个新去处 ——
 * 就是这里。所以这一页不是"锦上添花的调试功能"，而是**唯一的可见通道**：
 * 少了它，用户点了「开始运行」之后出了什么事就完全无从得知。
 *
 * 三个设计要点：
 * 1. 轮询用游标增量拉取（只拿新增的行），开销与日志总量无关；
 * 2. 自动滚动只在用户本来就贴底时才跟着滚 —— 否则他会发现自己
 *    正在往回翻的时候被硬拽到底部；
 * 3. 全部状态放在闭包外的一个对象里，重新渲染页面不会把游标搞丢。
 */
const logsState = {
  cursor: null,   // 上次拿到的最大 seq
  lines: [],      // 已显示的行
  timer: null,    // 轮询句柄
  level: '',      // 级别筛选：'' = 全部
};

/** 级别对应的颜色，与终端里的观感保持一致（WARN 黄、ERROR 红）。 */
const LOG_LEVELS = ['', 'INFO', 'WARN', 'ERROR', 'DEBUG', 'TRACE'];

function logLevelClass(lv) {
  switch (lv) {
    case 'ERROR': return 'lv-error';
    case 'WARN': return 'lv-warn';
    case 'DEBUG': case 'TRACE': return 'lv-dim';
    default: return 'lv-info';
  }
}

async function renderLogs() {
  // 每次进页面都重新开始：先拿一段上下文（最近的 500 行），
  // 而不是从空白开始 —— 用户点进来通常是想看"刚才发生了什么"
  logsState.cursor = null;
  logsState.lines = [];

  view.innerHTML = `
  <div class="card">
    <h2>${esc(t('lg.title'))}</h2>
    <p class="hint">${t('lg.hint')}</p>

    <div class="log-toolbar">
      <label class="log-filter">
        <span>${esc(t('lg.level'))}</span>
        <select id="log-level">
          ${LOG_LEVELS.map(l => `<option value="${l}">${
            l === '' ? esc(t('lg.level_all')) : l}</option>`).join('')}
        </select>
      </label>
      <label class="log-check">
        <input type="checkbox" id="log-follow" checked>
        <span>${esc(t('lg.follow'))}</span>
      </label>
      <span class="log-count" id="log-count"></span>
      <span style="flex:1"></span>
      <button class="btn ghost sm" id="log-reload">${esc(t('lg.reload'))}</button>
      <button class="btn ghost sm" id="log-copy">${esc(t('lg.copy'))}</button>
      <button class="btn ghost sm" id="log-clear">${esc(t('lg.clear'))}</button>
    </div>

    <div class="log-view" id="log-view"><div class="empty">${esc(t('common.loading'))}</div></div>

    <p class="hint" id="log-path" style="margin-top:10px"></p>
  </div>`;

  await loadLogsInto();
  scheduleLogsPoll();
}

/** 初次（或手动刷新）载入：清空并重新拉取整段。 */
async function loadLogsInto() {
  logsState.cursor = null;
  logsState.lines = [];
  const r = await api('/api/logs?limit=500');
  if (!r.ok) {
    paintLogs(`<div class="empty">${esc(r.error || t('common.unknown'))}</div>`);
    return;
  }
  logsState.lines = r.lines || [];
  logsState.cursor = r.cursor;
  paintLogPath(r);
  paintLogs();
}

/** 按当前筛选重绘整个日志区。 */
function paintLogs(errorHtml) {
  const box = document.getElementById('log-view');
  if (!box) return;
  if (errorHtml) { box.innerHTML = errorHtml; return; }

  const lv = logsState.level;
  const shown = lv ? logsState.lines.filter(l => l.level === lv) : logsState.lines;

  // 贴底判断必须在替换 innerHTML **之前**做 —— 之后 scrollTop 已经归零，
  // 就永远判断不出"用户本来在看底部"
  const atBottom = box.scrollHeight - box.scrollTop - box.clientHeight < 40;
  const follow = document.getElementById('log-follow');
  const shouldFollow = follow ? follow.checked : true;

  if (!shown.length) {
    box.innerHTML = `<div class="empty">${
      esc(logsState.lines.length ? t('lg.filter_empty') : t('lg.empty'))}</div>`;
  } else {
    box.innerHTML = shown.map(l => `<div class="log-line ${logLevelClass(l.level)}">` +
      `<span class="log-time">${esc(l.time)}</span>` +
      `<span class="log-lv">${esc(l.level)}</span>` +
      `<span class="log-text">${esc(l.text)}</span></div>`).join('');
  }

  if (shouldFollow && atBottom) box.scrollTop = box.scrollHeight;

  const cnt = document.getElementById('log-count');
  if (cnt) {
    cnt.textContent = lv
      ? t('lg.count_filtered').replace('{n}', shown.length).replace('{m}', logsState.lines.length)
      : t('lg.count').replace('{n}', logsState.lines.length);
  }
}

/** 显示日志文件位置 —— 界面里看不到的往期内容在那里。 */
function paintLogPath(r) {
  const el = document.getElementById('log-path');
  if (!el) return;
  el.textContent = (r.file_exists ? t('lg.file') : t('lg.file_missing')) + ' ' + (r.path || '');
}

/** 增量拉取：只要游标之后的新行。 */
async function pollLogs() {
  if (current !== 'logs') return; // 切走了就别再占着接口
  const q = logsState.cursor === null || logsState.cursor === undefined
    ? '/api/logs?limit=500'
    : `/api/logs?since=${logsState.cursor}`;
  try {
    const r = await api(q);
    if (r.ok) {
      // 后端说缓冲被清空过：我们手里的游标已经作废，整段重来
      if (r.cleared) {
        logsState.lines = [];
        logsState.cursor = null;
        await loadLogsInto();
      } else if ((r.lines || []).length) {
        logsState.lines = logsState.lines.concat(r.lines);
        // 本地也留个上限，防止长时间挂在页面上把内存堆起来
        if (logsState.lines.length > 3000) {
          logsState.lines = logsState.lines.slice(-3000);
        }
        logsState.cursor = r.cursor;
        paintLogs();
      }
    }
  } catch { /* 一次拉不到不影响下次 */ }
  scheduleLogsPoll();
}

function scheduleLogsPoll() {
  if (logsState.timer) clearTimeout(logsState.timer);
  // 1 秒一次：日志要"实时"，但也没必要更密。开销很小（通常返回 0 行）。
  logsState.timer = setTimeout(pollLogs, 1000);
}

function stopLogsPoll() {
  if (logsState.timer) { clearTimeout(logsState.timer); logsState.timer = null; }
}

/** 绑定工具栏。单独一个函数是因为重新载入后要重绑。 */
function bindLogToolbar() {
  const lv = document.getElementById('log-level');
  if (lv) {
    lv.value = logsState.level;
    lv.addEventListener('change', () => { logsState.level = lv.value; paintLogs(); });
  }
  const re = document.getElementById('log-reload');
  if (re) re.addEventListener('click', () => loadLogsInto());

  const cp = document.getElementById('log-copy');
  if (cp) cp.addEventListener('click', async () => {
    // 复制的是**当前看到的**（含筛选），所见即所得
    const lvSel = logsState.level;
    const shown = lvSel ? logsState.lines.filter(l => l.level === lvSel) : logsState.lines;
    const text = shown.map(l => l.time + ' ' + l.level + ' ' + l.text).join('\n');
    try {
      await navigator.clipboard.writeText(text);
      show(esc(t('lg.copied')));
    } catch {
      // 剪贴板 API 在非安全上下文里会被拒（虽然这里是 127.0.0.1）
      const ta = document.createElement('textarea');
      ta.value = text; document.body.appendChild(ta); ta.select();
      try { document.execCommand('copy'); show(esc(t('lg.copied'))); }
      catch { show(esc(t('lg.copy_failed')), 'err'); }
      ta.remove();
    }
  });

  const cl = document.getElementById('log-clear');
  if (cl) cl.addEventListener('click', async () => {
    await api('/api/logs/clear', {});
    logsState.lines = [];
    logsState.cursor = null;
    paintLogs();
    show(esc(t('lg.cleared')));
  });
}


const PAGES = {
  overview: ['nav.overview', renderOverview],
  run: ['nav.run', renderRun],
  model: ['nav.model', renderModel],
  push: ['nav.push', renderPush],
  timetable: ['nav.timetable', renderTimetable],
  schedule: ['nav.schedule', renderSchedule],
  record: ['nav.record', renderRecord],
  jobs: ['nav.jobs', renderJobs],
  logs: ['nav.logs', renderLogs],
};

let current = 'overview';

async function go(page) {
  // 离开日志页就停掉轮询：否则它会一直在后台拉，
  // 而且 current 已经变了，白费力气
  if (current === 'logs' && page !== 'logs') stopLogsPoll();

  current = page;
  const [titleKey, fn] = PAGES[page] || PAGES.overview;
  titleEl.textContent = t(titleKey);
  document.querySelectorAll('.nav-item').forEach(b =>
    b.classList.toggle('active', b.dataset.page === page));
  view.innerHTML = `<div class="empty">${esc(t('common.loading'))}</div>`;
  try {
    await fn();
    // 日志页的工具栏在 renderLogs 里生成，绑定放在渲染之后
    if (page === 'logs') bindLogToolbar();
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

/* ------------------------------------------------ 去掉「浏览器套壳」痕迹 */

/**
 * 这是一个桌面应用，不是网页。
 *
 * 窗口本体已经是 `--app=` 模式（没有地址栏/标签栏），F12 之类的开发者工具
 * 在启动参数里也已经关掉了。这里再补一层**页面内**的拦截：
 *
 * - `F12` / `Ctrl+Shift+I` / `Ctrl+Shift+J` / `Ctrl+U`（查看源代码）
 * - 右键菜单（这是最像浏览器的一处：原生菜单里全是「重新加载/另存为/检查」）
 *
 * 为什么明知道启动参数已经关了还要拦一遍：
 * 启动参数只对我们自己拉起的那条路径有效（比如用户手动把地址粘进浏览器时
 * 就不生效了）。页面内这层是兜底，代价接近零。
 *
 * 注意作用范围：只拦这几个组合键，**不是**全面禁用 F5/输入框右键 ——
 * 过度拦截会让页面里正常的文本复制粘贴都不可用，那才是真的惹人烦。
 */
(function blockBrowserChrome() {
  window.addEventListener('keydown', e => {
    const k = (e.key || '').toLowerCase();
    // F12
    if (e.key === 'F12') { e.preventDefault(); return; }
    // Ctrl+Shift+I / J / C（开发者工具、控制台、元素选择）
    if (e.ctrlKey && e.shiftKey && ['i', 'j', 'c'].includes(k)) { e.preventDefault(); return; }
    // Ctrl+U 查看源代码
    if (e.ctrlKey && !e.shiftKey && k === 'u') { e.preventDefault(); }
  }, true); // 用捕获阶段：抢在页面其它监听器之前吃掉

  window.addEventListener('contextmenu', e => {
    // 允许在输入框/文本域里用右键（复制、粘贴是正经需求），
    // 其余位置一律屏蔽 —— 那些位置的原生菜单只有浏览器项，没有应用项。
    const el = e.target;
    const isTextInput =
      el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.isContentEditable);
    if (!isTextInput) e.preventDefault();
  });
})();

/* ------------------------------------------------------------ 依赖缺失提示 */

/**
 * 启动时检查外部依赖（ffmpeg / whisper / 语音模型）是否齐备。
 *
 * 为什么要有这个：这几个组件加起来一百多 MB，不可能塞进安装包，
 * 所以部署到一台新的一体机上时可能是缺的。缺了以后程序的**表现**很迷惑 ——
 * 点了录制没反应、转写按钮转半天不出字 —— 用户完全不知道是缺文件。
 * 与其让人去猜，不如启动时直接说清楚缺什么、点一下就补。
 *
 * 提醒策略（与后端 /api/deps 的 need_notice 一致）：
 * - 缺**必需**项（ffmpeg）：每次都提示，因为程序真的没法用；
 * - 缺**可选**项（whisper / 模型）：只提示一次，之后走界面上的按钮补，
 *   不再反复弹窗骚扰。
 */
async function checkDeps() {
  let st;
  try {
    st = await api('/api/deps');
  } catch { return; }              // 查不出来就当没缺，别因为检查失败拦住用户
  if (!st || !st.ok) return;

  const missing = (st.items || []).filter(i => !i.ready);
  if (!missing.length) return;      // 都齐了，静默通过

  // 只有「这次该提醒的」才弹。全是可选且已提醒过 -> need_notice 为 0。
  if (!st.need_notice) return;

  showDepsModal(st, missing);
}

/**
 * 依赖缺失弹窗。
 *
 * 点「确定」后自动补齐 —— 这是需求的核心：用户不需要知道 ffmpeg 是干什么的、
 * 该放到哪个目录，只要点一下。
 */
function showDepsModal(st, missing) {
  const box = document.createElement('div');
  box.className = 'modal-mask';
  box.id = 'deps-modal';

  const required = missing.filter(i => i.required);
  const optional = missing.filter(i => !i.required);

  const row = i => `
    <li class="dep-row${i.required ? ' dep-req' : ''}">
      <div class="dep-name">${esc(i.label)}
        <span class="dep-tag">${i.required ? t('deps.tag_required') : t('deps.tag_optional')}</span>
      </div>
      <div class="dep-why">${esc(i.reason)}${i.reason && i.consequence ? ' · ' : ''}${esc(i.consequence)}</div>
    </li>`;

  box.innerHTML = `
    <div class="modal-box">
      <div class="modal-title">${esc(t('deps.title'))}</div>
      <div class="modal-body">
        <p class="dep-lead">${esc(t('deps.lead'))}</p>
        <ul class="dep-list">
          ${required.map(row).join('')}
          ${optional.map(row).join('')}
        </ul>
        <div class="dep-note" id="deps-note">${esc(t('deps.download_note'))}</div>
        <div class="dep-progress" id="deps-progress" hidden></div>
      </div>
      <div class="modal-actions">
        <button class="btn primary" id="deps-ok">${esc(t('deps.ok'))}</button>
        <button class="btn" id="deps-later">${esc(t('deps.later'))}</button>
      </div>
    </div>`;
  document.body.appendChild(box);

  const noteEl = box.querySelector('#deps-note');
  const progEl = box.querySelector('#deps-progress');
  const okBtn = box.querySelector('#deps-ok');
  const laterBtn = box.querySelector('#deps-later');

  laterBtn.addEventListener('click', async () => {
    // 记住"可选项已经提醒过"，下次启动不再为它们弹窗。
    // 必需项不受影响 —— 那个必须每次都提醒。
    try { await api('/api/deps/dismiss', {}); } catch { /* 记不住就下次再问 */ }
    box.remove();
  });

  okBtn.addEventListener('click', async () => {
    okBtn.disabled = true;
    laterBtn.disabled = true;
    okBtn.textContent = t('deps.fetching');
    progEl.hidden = false;
    progEl.textContent = t('deps.starting');

    try {
      const r = await api('/api/deps/fetch', {});
      if (!r.ok) {
        progEl.textContent = r.error || t('deps.failed');
        okBtn.disabled = false;
        laterBtn.disabled = false;
        okBtn.textContent = t('deps.retry');
        return;
      }
    } catch (e) {
      progEl.textContent = String(e);
      okBtn.disabled = false;
      laterBtn.disabled = false;
      return;
    }

    // 轮询进度。800ms 一次：下载是分钟级的事，查太勤没必要。
    const timer = setInterval(async () => {
      let p;
      try { p = await api('/api/deps/fetch'); } catch { return; }
      if (p && p.message) progEl.textContent = p.message;
      if (p && !p.running) {
        clearInterval(timer);
        // 补完复查一遍：真的齐了才允许关窗，免得用户以为补上了其实还缺
        let after;
        try { after = await api('/api/deps'); } catch { after = null; }
        const stillMissing = ((after && after.items) || []).filter(i => !i.ready);
        if (!stillMissing.length) {
          progEl.textContent = t('deps.done');
          okBtn.textContent = t('deps.close');
          okBtn.disabled = false;
          okBtn.onclick = () => box.remove();
          laterBtn.hidden = true;
          box.querySelector('#deps-note').textContent = t('deps.done_note');
        } else {
          progEl.textContent = (p.message || '') + ' · ' + t('deps.still_missing');
          okBtn.disabled = false;
          laterBtn.disabled = false;
          okBtn.textContent = t('deps.retry');
          okBtn.onclick = null;
        }
      }
    }, 800);
  });
}

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
  // 依赖检查放在最后：界面已经能用了再弹窗，不让检查拖慢首屏。
  // 而且它**不 await**——就算检查卡在网络超时上，界面也照常能用。
  checkDeps();
})();
