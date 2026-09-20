/* ============================================================================
   VibeClassAgent 界面脚本（原生 JS，无框架、无构建步骤）
   ----------------------------------------------------------------------------
   为什么不用 React/Vue：这是个单机工具界面，交互量很小；引一套框架就得配
   npm + 打包，而发布形态是"拷个文件夹过去双击"。原生 JS 够用，且改一行
   刷新就能看到，调试成本最低。
   ========================================================================== */

const TOKEN = new URLSearchParams(location.search).get('t') || '';
const view = document.getElementById('view');
const titleEl = document.getElementById('page-title');

/** 当前页面缓存的数据，切换页面时不用重新拉。 */
const state = { status: null, llm: null, push: null, providers: [], timetable: null, schedule: null };

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
  const t = document.createElement('template');
  t.innerHTML = html.trim();
  return t.content.firstElementChild;
}

/* ------------------------------------------------------------------ 概览 */

async function renderOverview() {
  const s = await api('/api/status');
  state.status = s;
  if (!s.ok) { view.innerHTML = `<div class="empty">读取状态失败：${esc(s.error)}</div>`; return; }

  paintDots(s);

  view.innerHTML = '';
  const grid = el(`<div class="grid"></div>`);
  const stats = [
    ['模型 API', s.llm.ready ? '已配置' : '未配置', s.llm.ready ? 'ok' : 'bad', s.llm.model || '—'],
    ['推送渠道', s.push.ready ? s.push.provider : '未配置', s.push.ready ? 'ok' : 'bad', s.push.target || '—'],
    ['本地转写', s.whisper_ready ? '就绪' : '缺失', s.whisper_ready ? 'ok' : 'bad', 'whisper.cpp'],
    ['作业', `${s.jobs.total}`, '', `待处理 ${s.jobs.pending}`],
  ];
  for (const [k, v, cls, sub] of stats) {
    grid.appendChild(el(`<div class="stat">
      <div class="k">${esc(k)}</div>
      <div class="v ${cls}">${esc(v)}</div>
      <div class="k" style="margin:6px 0 0">${esc(sub)}</div>
    </div>`));
  }
  view.appendChild(grid);

  const paths = el(`<div class="card">
    <h2>位置</h2>
    <p class="hint">数据和配置都在程序文件夹里，整个文件夹拷走就能换机器。</p>
    <table>
      <tr><th style="width:120px">配置</th><td>${esc(s.config_root)}</td></tr>
      <tr><th>数据</th><td>${esc(s.data_root)}</td></tr>
    </table>
  </div>`);
  view.appendChild(paths);

  const next = el(`<div class="card">
    <h2>怎么开始用</h2>
    <p class="hint">三步走完就能按课表自动录课。</p>
    <div class="note">
      <b>1. 填模型 API</b> —— 左侧「模型 API」，选服务商、粘 Key、保存。<br>
      <b>2. 配推送</b> —— 左侧「推送」，选渠道填参数，点「发送测试消息」确认能收到。<br>
      <b>3. 填时间表与课表</b> —— 左侧对应页面，导入或手工加。<br>
      都齐了之后，程序会在上课时段自动录制、在午休与晚餐时段自动处理并推送。
    </div>
  </div>`);
  view.appendChild(next);
}

function paintDots(s) {
  document.getElementById('pill-profile').textContent = s.profile || 'default';
  const set = (id, on) => {
    const d = document.getElementById(id);
    d.className = 'dot ' + (on ? 'on' : 'off');
  };
  set('dot-llm', s.llm.ready);
  set('dot-push', s.push.ready);
  set('dot-stt', s.whisper_ready);
}

/* ------------------------------------------------------------------ 模型 */

async function renderModel() {
  const [cfg, prov] = await Promise.all([api('/api/config/llm'), api('/api/providers')]);
  state.llm = cfg;
  state.providers = prov.providers || [];
  if (!cfg.ok) { view.innerHTML = `<div class="empty">${esc(cfg.error)}</div>`; return; }

  const opts = ['<option value="">— 手动填写地址 —</option>']
    .concat(state.providers.map(p =>
      `<option value="${esc(p.id)}" data-url="${esc(p.base_url)}" ${p.id === cfg.provider ? 'selected' : ''}>${esc(p.name)}${p.note ? ' · ' + esc(p.note) : ''}</option>`))
    .join('');

  view.innerHTML = '';
  const card = el(`<div class="card">
    <h2>要点提取用的模型</h2>
    <p class="hint">
      只有在转写成文字之后，才会把文本发给这个模型 —— 录屏与录音本身不会上传。<br>
      不配也能跑：提取会降级成「原文摘要」，文档照常生成。
    </p>

    <label class="field"><span>服务商</span>
      <select id="prov">${opts}</select>
    </label>

    <label class="field"><span>接口地址（OpenAI 兼容）</span>
      <input type="text" id="base" value="${esc(cfg.base_url)}" placeholder="https://api.deepseek.com/v1">
    </label>

    <label class="field"><span>模型名</span>
      <div class="row">
        <input type="text" id="model" value="${esc(cfg.model)}" placeholder="deepseek-chat">
        <button class="btn sm" id="btn-models">拉取列表</button>
      </div>
      <div class="note" id="models-note" style="display:none"></div>
    </label>

    <label class="field"><span>API Key ${cfg.key_set ? '<span class="tag ok">已设置</span>' : '<span class="tag bad">未设置</span>'}</span>
      <input type="password" id="key" placeholder="留空表示不改；填了会写进 secrets.env（不入库）">
    </label>

    <label class="field"><span>DeepSeek 错峰</span>
      <label style="display:flex;align-items:center;gap:8px;font-size:13px;color:var(--text-dim)">
        <input type="checkbox" id="defer" style="width:auto" ${cfg.defer_on_peak ? 'checked' : ''}>
        高峰时段自动积压，等闲时（半价）再跑
      </label>
    </label>

    <div class="row">
      <button class="btn primary" id="btn-save">保存</button>
      <span class="spacer"></span>
    </div>

    <div class="note">
      高峰 = 工作日 UTC 01:00–04:00 与 06:00–10:00，即<b>北京时间 09:00–12:00 与 14:00–18:00</b>
      （正好是上课时间）；其余时间半价。开着它，撞上高峰的处理任务会先积压、进闲时自动补跑。
    </div>
  </div>`);
  view.appendChild(card);

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
      toast('已保存', '写入字段：' + (r.wrote || []).join('、'));
      document.getElementById('key').value = '';
      renderModel();
      refreshDots();
    } else {
      toast('保存失败', r.error || '未知原因', 'err');
    }
  });

  document.getElementById('btn-models').addEventListener('click', async () => {
    const note = document.getElementById('models-note');
    note.style.display = 'block';
    note.textContent = '正在拉取…';
    const r = await api('/api/models', {});
    if (!r.ok) { note.textContent = '拉取失败：' + (r.error || ''); return; }
    if (!r.models || !r.models.length) { note.textContent = '服务端没有返回模型列表，请手动填写模型名。'; return; }
    note.innerHTML = '可用模型（点一个填进去）：<br>' + r.models.slice(0, 40)
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
  { id: 'wecom', name: '企业微信机器人', needs: ['endpoint'] },
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

  view.innerHTML = '';
  const card = el(`<div class="card">
    <h2>课后文档发到哪里</h2>
    <p class="hint">
      推荐 <b>OneBot 11</b>：本机跑一个 <a href="https://github.com/NapNeko/NapCatQQ" target="_blank" style="color:#7fd3ba">NapCat</a>
      登录 QQ，它开 HTTP 服务，这里填地址和群号即可（能发文件）。<br>
      最省事的是<b>企业微信机器人</b>：只要一个 Webhook 地址，不用装任何东西。
    </p>

    <label class="field"><span>渠道</span><select id="prov">${opts}</select></label>

    <div id="dyn"></div>

    <div class="row">
      <button class="btn primary" id="btn-save">保存</button>
      <button class="btn" id="btn-test">发送测试消息</button>
      <span class="spacer"></span>
    </div>

    <div class="note" id="test-note" style="display:none"></div>

    <div class="note">
      <b>OneBot / NapCat</b>：确认 NapCat 已登录 QQ 且开了 HTTP 服务（默认 <code>http://127.0.0.1:3000</code>），
      token 与这里填的一致；发群要选「群」。<br>
      <b>403</b> = token 不对；<b>connection refused</b> = 服务没起来或端口不对；<br>
      显示成功但群里没消息 → 多半是 target 填成了 QQ 号，发群必须选「群」。
    </div>
  </div>`);
  view.appendChild(card);

  const dyn = document.getElementById('dyn');
  const drawFields = () => {
    const id = document.getElementById('prov').value;
    const ch = CHANNELS.find(c => c.id === id) || { needs: [] };
    const has = n => ch.needs.includes(n);
    dyn.innerHTML = `
      ${has('endpoint') ? `<label class="field"><span>服务地址 / Webhook URL</span>
        <input type="text" id="endpoint" value="${esc(cfg.endpoint)}" placeholder="http://127.0.0.1:3000"></label>` : ''}
      ${has('target') ? `<label class="field"><span>目标</span>
        <div class="row">
          <input type="text" id="target" value="${esc(cfg.target)}" placeholder="群号或 QQ 号">
          <select id="ttype" style="width:130px">
            <option value="group" ${cfg.target_type === 'group' ? 'selected' : ''}>群</option>
            <option value="private" ${cfg.target_type === 'private' ? 'selected' : ''}>私聊</option>
          </select>
        </div></label>` : ''}
      ${has('qq_app_id') ? `<label class="field"><span>AppID ${cfg.qq_appid_set ? '<span class="tag ok">已设置</span>' : ''}</span>
        <input type="text" id="qq_app_id" placeholder="QQ 开放平台 AppID"></label>` : ''}
      ${has('qq_app_secret') ? `<label class="field"><span>AppSecret ${cfg.qq_secret_set ? '<span class="tag ok">已设置</span>' : ''}</span>
        <input type="password" id="qq_app_secret" placeholder="QQ 开放平台 AppSecret"></label>` : ''}
      ${has('token') ? `<label class="field"><span>访问令牌 ${cfg.token_set ? '<span class="tag ok">已设置</span>' : '<span class="tag">未设置</span>'}</span>
        <input type="password" id="token" placeholder="留空表示不改；OneBot 里设的 access_token"></label>` : ''}
    `;
  };
  drawFields();
  document.getElementById('prov').addEventListener('change', drawFields);

  const collect = () => {
    const p = { provider: document.getElementById('prov').value };
    for (const f of ['endpoint', 'target', 'target_type', 'qq_app_id', 'qq_app_secret', 'token']) {
      const node = document.getElementById(f);
      if (node && node.value.trim()) p[f] = node.value.trim();
    }
    return p;
  };

  document.getElementById('btn-save').addEventListener('click', async () => {
    const r = await api('/api/config/push', collect());
    if (r.ok) { toast('已保存', '写入字段：' + (r.wrote || []).join('、')); renderPush(); refreshDots(); }
    else toast('保存失败', r.error || '没有可写的字段', 'err');
  });

  document.getElementById('btn-test').addEventListener('click', async () => {
    const note = document.getElementById('test-note');
    note.style.display = 'block';
    note.textContent = '正在发送…';
    const r = await api('/api/push/test', {});
    if (r.ok) {
      note.innerHTML = `<b style="color:#7fd3ba">发送成功</b>（消息 id = ${esc(r.message_id || '-')}）。
        去群里看一眼，应该收到一条「VibeClassAgent 推送测试」。`;
      toast('推送成功', '消息已发出');
    } else {
      note.innerHTML = `<b style="color:#e05c5c">发送失败</b><br><code>${esc(r.error || '未知原因')}</code>`;
      toast('推送失败', r.error || '', 'err');
    }
  });
}

/* ---------------------------------------------------------------- 时间表 */

async function renderTimetable() {
  const d = await api('/api/timetable');
  state.timetable = d;
  if (!d.ok) { view.innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }

  const slots = d.slots || [];
  view.innerHTML = '';
  const card = el(`<div class="card">
    <h2>作息时间表</h2>
    <p class="hint">
      把一天分成「上课」与「休息」两类时段。程序据此做两件事：<br>
      · 上课时段录屏录音；<br>
      · <b>自动挑出名字带「午休 / 晚餐 / 放学」的休息段</b>作为课后处理窗口
        （这也正好是 DeepSeek 的闲时）。
    </p>
    <div class="slot-row slot-head">
      <div>#</div><div>开始</div><div>结束</div><div>类型</div><div>名称</div><div></div>
    </div>
    <div id="slots"></div>
    <div class="row" style="margin-top:12px">
      <button class="btn sm" id="btn-add">+ 加一段</button>
      <span class="spacer"></span>
      <button class="btn primary" id="btn-save">保存</button>
    </div>
    <div class="note" id="win-note" style="display:none"></div>
  </div>`);
  view.appendChild(card);

  const host = document.getElementById('slots');
  const addRow = (s) => {
    const row = el(`<div class="slot-row">
      <div style="color:var(--text-dim2);font-family:var(--mono);font-size:12px;padding-top:8px">${host.children.length + 1}</div>
      <input type="text" value="${esc(s.start || '08:00')}" placeholder="08:00">
      <input type="text" value="${esc(s.end || '08:45')}" placeholder="08:45">
      <select>
        <option value="class" ${s.kind === 'class' ? 'selected' : ''}>上课</option>
        <option value="break" ${s.kind === 'break' ? 'selected' : ''}>休息</option>
      </select>
      <input type="text" value="${esc(s.name || '')}" placeholder="例：午餐午休">
      <button class="btn sm danger">×</button>
    </div>`);
    row.querySelector('button').addEventListener('click', () => {
      row.remove();
      [...host.children].forEach((r, i) => r.firstElementChild.textContent = i + 1);
    });
    host.appendChild(row);
  };
  slots.forEach(addRow);
  if (!slots.length) { addRow({ start: '08:00', end: '08:45', kind: 'class', name: '' }); }

  document.getElementById('btn-add').addEventListener('click', () => addRow({}));

  document.getElementById('btn-save').addEventListener('click', async () => {
    const list = [...host.children].map((r, i) => {
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
    const payload = {
      timetables: [{
        id: 'default', name: d.name || '我的作息', is_active: true, source: 'manual',
        slots: list,
      }],
    };
    const r = await api('/api/timetable', payload);
    if (r.ok) {
      toast('已保存', `共 ${r.slots} 段`);
      const note = document.getElementById('win-note');
      const w = r.windows || [];
      note.style.display = 'block';
      note.innerHTML = w.length
        ? '<b>自动推导出的处理窗口：</b><br>' + w.map(x =>
            `· ${esc(x.name)}（${esc(x.start)}–${esc(x.end)}，scope=${esc(x.scope)}）`).join('<br>')
        : '<b style="color:#d9a343">没有推导出处理窗口</b>：时间表里缺少名字带「午休 / 晚餐 / 放学」的休息段。';
    } else toast('保存失败', r.error || '', 'err');
  });
}

/* ---------------------------------------------------------------- 课表 */

const DAYS = [['Mon', '周一'], ['Tue', '周二'], ['Wed', '周三'], ['Thu', '周四'],
              ['Fri', '周五'], ['Sat', '周六'], ['Sun', '周日']];
const CYCLES = [['every', '每周'], ['odd', '单周'], ['even', '双周']];

async function renderSchedule() {
  const d = await api('/api/schedule');
  state.schedule = d;
  if (!d.ok) { view.innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }

  view.innerHTML = '';
  const card = el(`<div class="card">
    <h2>课程表</h2>
    <p class="hint">
      决定「哪节课要录」。单双周可以排在同一时段，程序不会当成冲突。
      导入后记得检查一下时间是否落在上面的上课时段内。
    </p>
    <div class="row wrap" style="margin-bottom:14px">
      <button class="btn sm" id="btn-add">+ 加一条</button>
      <button class="btn sm" id="btn-csv">导出 CSV 模板</button>
      <span class="spacer"></span>
      <span style="font-size:12px;color:var(--text-dim2)" id="cnt"></span>
      <button class="btn primary" id="btn-save">保存</button>
    </div>
    <div id="rows"></div>
    <div class="note" id="save-note" style="display:none"></div>
  </div>`);
  view.appendChild(card);

  const host = document.getElementById('rows');
  const addRow = (e) => {
    const row = el(`<div class="slot-row" style="grid-template-columns:110px 92px 92px 1fr 110px 74px 34px">
      <select>${DAYS.map(([v, n]) => `<option value="${v}" ${e.day === v ? 'selected' : ''}>${n}</option>`).join('')}</select>
      <input type="text" value="${esc(e.start || '08:00')}">
      <input type="text" value="${esc(e.end || '08:45')}">
      <input type="text" value="${esc(e.course || '')}" placeholder="课程名">
      <input type="text" value="${esc(e.teacherId || 't1')}" placeholder="教师 id">
      <select style="width:74px">${CYCLES.map(([v, n]) => `<option value="${v}" ${(e.cycle || 'every') === v ? 'selected' : ''}>${n}</option>`).join('')}</select>
      <button class="btn sm danger">×</button>
    </div>`);
    row.querySelector('button').addEventListener('click', () => { row.remove(); upd(); });
    host.appendChild(row);
    upd();
  };
  const upd = () => {
    document.getElementById('cnt').textContent = `共 ${host.children.length} 条`;
  };
  (d.entries || []).forEach(addRow);

  document.getElementById('btn-add').addEventListener('click', () => addRow({}));

  document.getElementById('btn-csv').addEventListener('click', () => {
    const rows = [[...DAYS.map(d => d[1]), '节次', '开始', '结束', '课程', '教师', '周次'].join(',')];
    [...host.children].forEach((r, i) => {
      const ins = r.querySelectorAll('input');
      const sels = r.querySelectorAll('select');
      const day = DAYS.find(([v]) => v === sels[0].value)[1];
      rows.push([day, i + 1, ins[0].value, ins[1].value, ins[2].value, ins[3].value, sels[1].value].join(','));
    });
    const blob = new Blob(['\ufeff' + rows.join('\n')], { type: 'text/csv;charset=utf-8' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = 'schedule.csv';
    a.click();
    toast('已导出', '可以用 Excel 打开编辑');
  });

  document.getElementById('btn-save').addEventListener('click', async () => {
    const entries = [...host.children].map((r, i) => {
      const ins = r.querySelectorAll('input');
      const sels = r.querySelectorAll('select');
      return {
        day: sels[0].value,
        period: i + 1,
        start: ins[0].value.trim(),
        end: ins[1].value.trim(),
        course: ins[2].value.trim() || '未命名课程',
        teacherId: ins[3].value.trim() || 't1',
        record: true,
        cycle: sels[1].value,
      };
    });
    const teachers = [...new Set(entries.map(e => e.teacherId))]
      .map(id => ({ id, name: id, profile: 'default' }));
    const payload = {
      teachers,
      week_template: { cycle: 'every', entries },
      weekend_template: { source: 'new', entries: [] },
      overrides: [],
    };
    const r = await api('/api/schedule', payload);
    if (r.ok) {
      toast('已保存', `${r.entries} 条条目`);
      const note = document.getElementById('save-note');
      note.style.display = 'block';
      note.innerHTML = (r.issues && r.issues.length)
        ? '<b style="color:#d9a343">有几处要确认：</b><br>· ' + r.issues.map(esc).join('<br>· ')
        : '<b style="color:#7fd3ba">课表检查通过</b>，没有发现时间重叠或漏填教师。';
    } else toast('保存失败', r.error || '', 'err');
  });
}

/* ---------------------------------------------------------- 录制与文件 */

async function renderRecord() {
  const d = await api('/api/config/general');
  if (!d.ok) { view.innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }

  view.innerHTML = '';
  const card = el(`<div class="card">
    <h2>录制参数</h2>
    <p class="hint">默认值是按「占用尽量低」选的：720p / 8 帧 / 每 3 分钟一张截图。<br>
      一体机配置一般，不建议调高 —— 一节课 45 分钟大约 100 MB。</p>

    <div class="grid">
      <label class="field"><span>画质（高度，像素）</span>
        <input type="number" id="height" value="${d.record.height}"></label>
      <label class="field"><span>帧率</span>
        <input type="number" id="fps" value="${d.record.fps}"></label>
      <label class="field"><span>截图间隔（秒）</span>
        <input type="number" id="shot" value="${d.record.screenshot_interval_secs}"></label>
      <label class="field"><span>录像保留（小时）</span>
        <input type="number" id="ret" value="${d.cleanup.retention_hours}"></label>
    </div>

    <label class="field"><span>录音来源</span>
      <select id="audio">
        <option value="both" ${d.record.audio_source === 'both' ? 'selected' : ''}>系统声音 + 麦克风（推荐）</option>
        <option value="system" ${d.record.audio_source === 'system' ? 'selected' : ''}>仅系统声音</option>
        <option value="mic" ${d.record.audio_source === 'mic' ? 'selected' : ''}>仅麦克风</option>
      </select>
    </label>

    <label class="field"><span>桌面悬浮窗</span>
      <label style="display:flex;align-items:center;gap:8px;font-size:13px;color:var(--text-dim)">
        <input type="checkbox" id="ov" style="width:auto" ${d.overlay.enabled ? 'checked' : ''}> 下课时在屏幕角落显示提示
      </label>
    </label>

    <label class="field"><span>悬浮窗文字</span>
      <input type="text" id="ovtext" value="${esc(d.overlay.text)}"></label>

    <div class="row">
      <button class="btn primary" id="btn-save">保存</button>
      <span class="spacer"></span>
    </div>
  </div>`);
  view.appendChild(card);

  const files = el(`<div class="card">
    <h2>清理策略</h2>
    <p class="hint">
      从<b>推送成功那一刻</b>开始倒计时；推送一直失败就一直留着本地录像 ——
      宁可占点磁盘，也不能把没送出去的东西删掉。
    </p>
    <div class="row">
      <button class="btn" id="btn-clean">预览可清理的录像</button>
      <span class="spacer"></span>
    </div>
    <div class="note" id="clean-note" style="display:none"></div>
  </div>`);
  view.appendChild(files);

  document.getElementById('btn-save').addEventListener('click', async () => {
    const payload = {
      height: +document.getElementById('height').value,
      fps: +document.getElementById('fps').value,
      screenshot_interval_secs: +document.getElementById('shot').value,
      retention_hours: +document.getElementById('ret').value,
      audio_source: document.getElementById('audio').value,
      overlay_enabled: document.getElementById('ov').checked,
      overlay_text: document.getElementById('ovtext').value,
    };
    const r = await api('/api/config/general', payload);
    if (r.ok) toast('已保存', '写入字段：' + (r.wrote || []).join('、'));
    else toast('保存失败', r.error || '', 'err');
  });

  document.getElementById('btn-clean').addEventListener('click', async () => {
    const note = document.getElementById('clean-note');
    note.style.display = 'block';
    note.textContent = '正在扫描…';
    const r = await api('/api/clean/preview', {});
    if (!r.ok) { note.textContent = '扫描失败：' + (r.error || ''); return; }
    note.innerHTML = `扫描到 ${r.scanned} 项，可删除 ${r.deleting} 项，未到期跳过 ${r.skipped} 项。` +
      (r.deleting ? '<br><span style="color:#d9a343">真正删除请点下面的按钮。</span>' : '');
  });
}

/* ---------------------------------------------------------------- 作业 */

async function renderJobs() {
  const d = await api('/api/jobs');
  if (!d.ok) { view.innerHTML = `<div class="empty">${esc(d.error)}</div>`; return; }

  view.innerHTML = '';
  const card = el(`<div class="card">
    <h2>作业</h2>
    <p class="hint">每上完一节课生成一条。状态走完 <code>已录 → 已转写 → 已出文档 → 已推送</code> 就算结束。</p>
    <table>
      <thead><tr><th>日期</th><th>课程</th><th>状态</th><th>文档</th><th>备注</th></tr></thead>
      <tbody id="tb"></tbody>
    </table>
    <div class="empty" id="none" style="display:none">还没有作业。等上完一节课就有了。</div>
  </div>`);
  view.appendChild(card);

  const tb = document.getElementById('tb');
  for (const j of (d.jobs || [])) {
    tb.appendChild(el(`<tr>
      <td>${esc(j.date)}</td>
      <td class="mono-dim">${esc(j.course)}</td>
      <td><span class="tag ${j.state === 'PUSHED' ? 'ok' : ''}">${esc(j.state)}</span></td>
      <td class="mono-dim" style="font-size:11.5px">${esc(j.docx ? j.docx.split('\\').pop() : '—')}</td>
      <td class="mono-dim">${esc(j.last_error || '')}</td>
    </tr>`));
  }
  if (!(d.jobs || []).length) document.getElementById('none').style.display = 'block';
}

/* ---------------------------------------------------------------- 路由 */

const PAGES = {
  overview: ['概览', renderOverview],
  model: ['模型 API', renderModel],
  push: ['推送', renderPush],
  timetable: ['时间表', renderTimetable],
  schedule: ['课表', renderSchedule],
  record: ['录制与文件', renderRecord],
  jobs: ['作业', renderJobs],
};

let current = 'overview';

async function go(page) {
  current = page;
  const [title, fn] = PAGES[page] || PAGES.overview;
  titleEl.textContent = title;
  document.querySelectorAll('.nav-item').forEach(b =>
    b.classList.toggle('active', b.dataset.page === page));
  view.innerHTML = '<div class="empty">载入中…</div>';
  try {
    await fn();
  } catch (e) {
    view.innerHTML = `<div class="empty">出错了：${esc(e.message)}</div>`;
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

go('overview');
refreshDots();
