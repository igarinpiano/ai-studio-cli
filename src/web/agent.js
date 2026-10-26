// Injected into the AI Studio page. Reads the chat and drives the run
// settings the way a person would: by finding controls through their visible
// labels and ARIA attributes, and clicking or typing into them.
//
// Every selector list below is tried in order. AI Studio's markup changes from
// time to time; lists can be extended without rebuilding through
// <app dir>/web-selectors.json (same keys as SEL), which the CLI passes in as
// window.__aiscliOverrides.
(() => {
  const VERSION = 1;
  if (window.__aiscli && window.__aiscli.version === VERSION) return;

  const SEL = {
    prompt: [
      'ms-prompt-input-wrapper textarea',
      'ms-prompt-box textarea',
      'ms-chunk-input textarea',
      'footer textarea',
      'textarea[aria-label*="prompt" i]',
      'textarea[placeholder*="prompt" i]',
      'textarea[placeholder*="Start typing" i]',
      'div[contenteditable="true"][role="textbox"]',
    ],
    runButton: [
      'ms-run-button button',
      'button[aria-label="Run"]',
      'button[aria-label*="Run" i]',
      'button[type="submit"]',
    ],
    turn: ['ms-chat-turn', '[data-turn-role]', '.chat-turn-container'],
    modelText: ['ms-cmark-node', '.model-response-text', '.turn-content', 'ms-text-chunk'],
    thought: ['ms-thought-chunk', '.thought-panel', '[data-test-id="thoughts"]'],
    errors: [
      'mat-snack-bar-container',
      '.mat-mdc-snack-bar-container',
      'ms-callout[type="error"]',
      '[role="alert"]',
      '.error-message',
    ],
    dialog: ['mat-dialog-container', '[role="dialog"]', '[role="alertdialog"]'],
    overlay: ['.cdk-overlay-container', '[role="dialog"]', '[role="listbox"]', '[role="menu"]'],
    option: ['mat-option', '[role="option"]', '[role="menuitem"]', '[role="menuitemradio"]'],
    switch: ['button[role="switch"]', '[role="switch"]', 'input[type="checkbox"]', 'mat-slide-toggle button'],
    select: ['mat-select', '[role="combobox"]', 'select'],
    runSettingsToggle: [
      'button[aria-label*="Run settings" i]',
      'button[aria-label*="run settings" i]',
      'button[aria-label*="settings" i][aria-label*="toggle" i]',
    ],
    modelSelector: ['ms-model-selector', 'ms-model-selector-v3', '[data-test-id="model-selector"]'],
    addMedia: [
      'button[aria-label*="Insert" i]',
      'button[aria-label*="Add" i][aria-label*="media" i]',
      'button[aria-label*="Upload" i]',
      'ms-add-chunk-menu button',
    ],
    sourceLinks: ['ms-grounding-sources a[href]', 'ms-search-entry-point a[href]', '.grounding-sources a[href]'],
  };
  const LABEL = {
    runSettings: /^(Run settings|実行設定)$/i,
    tools: /^(Tools|ツール)$/i,
    advanced: /^(Advanced settings|詳細設定)$/i,
    thinking: /^(Thinking level|Thinking|思考レベル)$/i,
    media: /^(Media resolution|メディア解像度)$/i,
    system: /^(System instructions|システム指示)$/i,
    temperature: /^(Temperature|温度)$/i,
    top_p: /^(Top P|Top-P|トップ P)$/i,
    output_length: /^(Output length|Max output tokens|出力の長さ)$/i,
    stop: /^(Add stop sequence|Stop sequence|停止シーケンス)/i,
    search: /^(Grounding with Google Search|Google Search|Google 検索.*グラウンディング)$/i,
    maps: /^(Grounding with Google Maps|Google Maps|Google マップ.*グラウンディング)$/i,
    url_context: /^(URL context|URL コンテキスト)$/i,
    code_execution: /^(Code execution|コード実行)$/i,
    structured_output: /^(Structured outputs?|構造化出力)$/i,
    function_calling: /^(Function calling|関数呼び出し)$/i,
  };
  const MODEL_ID = /\b((?:gemini|gemma|learnlm|imagen|veo|lyria)-[a-z0-9][a-z0-9.\-]*)/i;

  const ov = window.__aiscliOverrides || {};
  for (const k of Object.keys(ov)) {
    if (Array.isArray(ov[k])) SEL[k] = ov[k].concat(SEL[k] || []);
  }

  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const $all = (sel, root) => {
    try {
      return Array.from((root || document).querySelectorAll(sel));
    } catch (e) {
      return [];
    }
  };
  const visible = (el) => !!el && el.isConnected && el.getClientRects().length > 0;
  const text = (el) => ((el && (el.innerText || el.textContent)) || '').replace(/\s+/g, ' ').trim();
  // Text with a space between every text node, so ids in adjacent elements
  // ("Gemini 3.8 Flash" + "gemini-3.8-flash") don't run together.
  const spacedText = (el) => {
    if (!el) return '';
    const parts = [];
    const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) parts.push(n.nodeValue);
    return parts.join(' ').replace(/\s+/g, ' ').trim();
  };
  const first = (sels, root, pred) => {
    for (const s of sels) {
      const el = $all(s, root).find((e) => visible(e) && (!pred || pred(e)));
      if (el) return el;
    }
    return null;
  };
  const allOf = (sels, root) => {
    const seen = new Set();
    const out = [];
    for (const s of sels) for (const e of $all(s, root)) if (!seen.has(e)) (seen.add(e), out.push(e));
    return out;
  };
  const click = (el) => {
    el.scrollIntoView({ block: 'center' });
    for (const type of ['pointerdown', 'mousedown', 'pointerup', 'mouseup']) {
      el.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }));
    }
    el.click();
  };
  const pressKey = (el, key) => {
    for (const type of ['keydown', 'keyup']) {
      el.dispatchEvent(new KeyboardEvent(type, { key, code: key, bubbles: true, cancelable: true }));
    }
  };
  const setNativeValue = (el, value) => {
    const proto = el.tagName === 'TEXTAREA' ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, value);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  };

  // The innermost visible element whose whole text matches `re`.
  const labelEl = (re, root) => {
    let best = null;
    for (const el of $all('label, span, div, h1, h2, h3, h4, h5, p, button, mat-panel-title, [role="heading"]', root)) {
      if (!visible(el)) continue;
      const t = text(el);
      if (t.length > 60 || !re.test(t)) continue;
      if (!best || best.contains(el)) best = el;
    }
    return best;
  };
  // The first control matching `sels` in the closest ancestor of the label that has one.
  const controlNear = (re, sels) => {
    const label = labelEl(re);
    if (!label) return null;
    for (let a = label, i = 0; a && i < 7; a = a.parentElement, i++) {
      const c = first(sels, a);
      if (c) return c;
    }
    return null;
  };

  const promptBox = () => first(SEL.prompt);
  const runButton = () =>
    first(SEL.runButton, null, (b) => /run|stop|実行|停止/i.test(text(b) + ' ' + (b.getAttribute('aria-label') || '')));
  const isStop = (b) => !!b && /\bstop\b|停止/i.test(text(b) + ' ' + (b.getAttribute('aria-label') || ''));

  const turns = () => {
    for (const s of SEL.turn) {
      const list = $all(s).filter((t) => !t.parentElement || !t.parentElement.closest(s));
      if (list.length) return list;
    }
    return [];
  };
  const roleOf = (turn, index) => {
    const r = turn.matches('[data-turn-role]') ? turn : turn.querySelector('[data-turn-role]');
    if (r) return /user/i.test(r.getAttribute('data-turn-role')) ? 'user' : 'model';
    const cls = [turn, ...turn.querySelectorAll('.chat-turn-container, [class*="turn"]')]
      .slice(0, 4)
      .map((e) => (typeof e.className === 'string' ? e.className : ''))
      .join(' ');
    if (/\buser\b/i.test(cls)) return 'user';
    if (/\bmodel\b/i.test(cls)) return 'model';
    return index % 2 === 0 ? 'user' : 'model';
  };

  // --- DOM to markdown --------------------------------------------------------
  const SKIP_TAGS = new Set(['BUTTON', 'MAT-ICON', 'SVG', 'STYLE', 'SCRIPT', 'NOSCRIPT', 'TEMPLATE', 'INPUT', 'MAT-TOOLTIP-COMPONENT']);
  const skip = (el) =>
    SKIP_TAGS.has(el.tagName) ||
    el.getAttribute('aria-hidden') === 'true' ||
    el.getAttribute('role') === 'toolbar' ||
    /material-(symbols|icons)|cdk-visually-hidden|actions?-(bar|container)|turn-footer/i.test(
      typeof el.className === 'string' ? el.className : ''
    );
  const isBlock = (el) => {
    const d = getComputedStyle(el).display;
    return d && !d.startsWith('inline') && d !== 'contents' && d !== 'none';
  };
  const codeLang = (pre) => {
    const code = pre.querySelector('code') || pre;
    const m = (code.className || '').match(/language-([\w+#-]+)/);
    if (m) return m[1];
    const block = pre.closest('ms-code-block, .code-block');
    if (block) {
      const header = block.querySelector('[class*="header"], .language, .code-block-language');
      const t = text(header).replace(/\b(content_copy|download|expand_(more|less)|copy|code)\b/gi, '').trim();
      if (t && t.length < 20) return t.split(' ')[0].toLowerCase();
    }
    return '';
  };

  function toMarkdown(root, opts) {
    const walk = (node, ctx) => {
      if (node.nodeType === Node.TEXT_NODE) {
        return ctx.pre ? node.nodeValue : node.nodeValue.replace(/\s+/g, ' ');
      }
      if (node.nodeType !== Node.ELEMENT_NODE) return '';
      const el = node;
      if (skip(el)) return '';
      if (ctx.excludeThoughts && SEL.thought.some((s) => el.matches(s))) return '';
      if (el.tagName === 'MAT-EXPANSION-PANEL-HEADER') return '';
      const kids = (c) => Array.from(el.childNodes).map((n) => walk(n, c || ctx)).join('');
      const tag = el.tagName;
      switch (tag) {
        case 'H1': case 'H2': case 'H3': case 'H4': case 'H5': case 'H6':
          return `\n\n${'#'.repeat(+tag[1])} ${kids().trim()}\n\n`;
        case 'P':
          return `\n\n${kids().trim()}\n\n`;
        case 'BR':
          return '\n';
        case 'HR':
          return '\n\n---\n\n';
        case 'STRONG': case 'B': {
          const t = kids();
          return t.trim() ? `**${t.trim()}**` : t;
        }
        case 'EM': case 'I': {
          const t = kids();
          return t.trim() ? `*${t.trim()}*` : t;
        }
        case 'CODE':
          return ctx.pre ? el.textContent : '`' + el.textContent + '`';
        case 'PRE': {
          const code = (el.querySelector('code') || el).textContent.replace(/\n+$/, '');
          return `\n\n\`\`\`${codeLang(el)}\n${code}\n\`\`\`\n\n`;
        }
        case 'A': {
          const t = kids().trim();
          const href = el.getAttribute('href') || '';
          if (!href || href.startsWith('javascript:')) return t;
          return t && t !== href ? `[${t}](${href})` : href;
        }
        case 'IMG': {
          const src = el.getAttribute('src') || '';
          return src.startsWith('data:') ? '' : `![${el.getAttribute('alt') || ''}](${src})`;
        }
        case 'UL': case 'OL': {
          const items = Array.from(el.children).filter((c) => c.tagName === 'LI');
          const start = +(el.getAttribute('start') || 1);
          const lines = items.map((li, i) => {
            const marker = tag === 'OL' ? `${start + i}. ` : '- ';
            const body = Array.from(li.childNodes)
              .map((n) => walk(n, { ...ctx, depth: (ctx.depth || 0) + 1 }))
              .join('')
              .trim()
              .replace(/\n{2,}/g, '\n')
              .replace(/\n/g, '\n' + ' '.repeat(marker.length));
            return marker + body;
          });
          const indent = ctx.depth ? '\n' : '\n\n';
          return indent + lines.join('\n') + '\n\n';
        }
        case 'BLOCKQUOTE':
          return '\n\n' + kids().trim().split('\n').map((l) => '> ' + l).join('\n') + '\n\n';
        case 'TABLE': {
          const rows = Array.from(el.querySelectorAll('tr')).map((tr) =>
            Array.from(tr.children).map((c) => walk(c, ctx).trim().replace(/\|/g, '\\|').replace(/\n+/g, ' '))
          );
          if (!rows.length) return '';
          const width = Math.max(...rows.map((r) => r.length));
          const line = (r) => '| ' + Array.from({ length: width }, (_, i) => r[i] || '').join(' | ') + ' |';
          const out = [line(rows[0]), '|' + ' --- |'.repeat(width), ...rows.slice(1).map(line)];
          return '\n\n' + out.join('\n') + '\n\n';
        }
        default: {
          if (tag === 'MS-CODE-BLOCK' || tag === 'MS-CODE-BLOCK-V2') {
            const pre = el.querySelector('pre');
            if (pre) return walk(pre, ctx);
          }
          const t = kids();
          return isBlock(el) ? `\n${t}\n` : t;
        }
      }
    };
    return walk(root, { excludeThoughts: !(opts && opts.thoughts), depth: 0 })
      .replace(/[ \t]+\n/g, '\n')
      .replace(/\n{3,}/g, '\n\n')
      .trim();
  }

  // The answer part of a model turn (without thoughts and buttons).
  const answerRoots = (turn) => {
    for (const s of SEL.modelText) {
      const list = $all(s, turn).filter(
        (e) => !SEL.thought.some((t) => e.closest(t)) && !(e.parentElement && e.parentElement.closest(s))
      );
      if (list.length) return list;
    }
    return [turn];
  };

  const expandedThoughts = new WeakSet();
  const thoughtsOf = (turn) => {
    const chunks = allOf(SEL.thought, turn).filter((c) => !c.parentElement || !SEL.thought.some((s) => c.parentElement.closest(s)));
    let out = '';
    for (const chunk of chunks) {
      // Thoughts are collapsed by default; open them once so their text is in the page.
      const header = chunk.querySelector('[aria-expanded="false"], mat-expansion-panel-header');
      if (header && header.getAttribute('aria-expanded') === 'false' && !expandedThoughts.has(chunk)) {
        expandedThoughts.add(chunk);
        click(header);
      }
      const t = toMarkdown(chunk, { thoughts: true }).replace(/^(Thoughts|Thinking)\b[^\n]*\n*/i, '').trim();
      if (t) out += (out ? '\n\n' : '') + t;
    }
    return out;
  };
  const sourcesOf = (turn) => {
    const seen = new Set();
    const out = [];
    for (const a of allOf(SEL.sourceLinks, turn)) {
      const href = a.href;
      if (!href || seen.has(href)) continue;
      seen.add(href);
      out.push({ title: text(a), uri: href });
    }
    return out;
  };

  const errorsNow = () =>
    allOf(SEL.errors)
      .filter(visible)
      .map(text)
      .filter((t) => t && t.length < 1000);

  const dialogText = () => {
    const d = first(SEL.dialog);
    return d ? text(d).slice(0, 400) : '';
  };

  // --- run settings -------------------------------------------------------------
  const ensureRunSettings = async () => {
    if (labelEl(LABEL.runSettings) || labelEl(LABEL.thinking) || labelEl(LABEL.temperature)) return true;
    const b = first(SEL.runSettingsToggle);
    if (!b) return false;
    click(b);
    await sleep(400);
    return true;
  };
  const expandSection = async (re) => {
    const l = labelEl(re);
    if (!l) return;
    for (let a = l, i = 0; a && i < 4; a = a.parentElement, i++) {
      const exp = a.getAttribute && a.getAttribute('aria-expanded');
      if (exp === 'false') {
        click(a);
        await sleep(250);
        return;
      }
      if (exp === 'true') return;
    }
  };
  const findControl = async (key, sels) => {
    await ensureRunSettings();
    let c = controlNear(LABEL[key], sels);
    if (c) return c;
    await expandSection(LABEL.tools);
    await expandSection(LABEL.advanced);
    return controlNear(LABEL[key], sels);
  };
  const switchState = (c) => {
    if (c.getAttribute('aria-checked') != null) return c.getAttribute('aria-checked') === 'true';
    if ('checked' in c) return !!c.checked;
    return /checked/.test(c.closest('mat-slide-toggle')?.className || '');
  };
  const optionsOpen = () => allOf(SEL.option).filter(visible);
  const pickOption = async (control, want) => {
    click(control);
    await sleep(300);
    const opts = optionsOpen();
    const norm = (s) => s.toLowerCase().replace(/\s+/g, ' ').trim();
    const target = opts.find((o) => norm(text(o)) === norm(want)) || opts.find((o) => norm(text(o)).startsWith(norm(want)));
    if (!target) {
      const names = opts.map(text);
      pressKey(document.activeElement || document.body, 'Escape');
      return { ok: false, error: `option "${want}" not found (available: ${names.join(', ') || 'none'})` };
    }
    click(target);
    await sleep(200);
    return { ok: true };
  };

  const currentModel = () => {
    const sel = first(SEL.modelSelector);
    const scope = sel || labelEl(LABEL.runSettings)?.closest('aside, section, div[class*="settings"], ms-run-settings') || document.body;
    const m = spacedText(scope).match(MODEL_ID);
    return m ? m[1].toLowerCase() : null;
  };

  const modelCard = () => {
    const sel = first(SEL.modelSelector);
    if (sel) return sel.querySelector('button, [role="button"]') || sel;
    const id = currentModel();
    if (!id) return null;
    // The smallest clickable element that shows the current model id.
    let best = null;
    for (const el of $all('button, [role="button"], [tabindex]')) {
      const t = text(el);
      if (visible(el) && spacedText(el).toLowerCase().includes(id) && t.length < 400 && (!best || t.length < text(best).length)) best = el;
    }
    return best;
  };

  const modelEntries = () => {
    const roots = allOf(SEL.overlay).filter(visible);
    const seen = new Map();
    for (const root of roots) {
      for (const el of $all('*', root)) {
        if (!visible(el) || el.children.length > 12) continue;
        const t = spacedText(el);
        if (t.length > 300) continue;
        const m = t.match(MODEL_ID);
        if (!m) continue;
        const id = m[1].toLowerCase();
        let clickable = el.closest('button, [role="option"], [role="button"], [role="menuitem"], li, mat-list-option, [tabindex]');
        if (!clickable || !root.contains(clickable)) clickable = el;
        // One model per clickable entry: skip containers listing several.
        const ids = spacedText(clickable).match(new RegExp(MODEL_ID.source, 'gi')) || [];
        if (new Set(ids.map((x) => x.toLowerCase())).size > 1) continue;
        const prev = seen.get(id);
        if (!prev || clickable.contains(prev.el)) {
          const lines = (clickable.innerText || '').split('\n').map((s) => s.trim()).filter(Boolean);
          const name = lines.find((l) => !MODEL_ID.test(l)) || id;
          const description = lines.filter((l) => l !== name && !MODEL_ID.test(l)).join(' ');
          seen.set(id, { el: clickable, id, name, description });
        }
      }
    }
    return Array.from(seen.values());
  };

  const api = {
    version: VERSION,

    ready() {
      const url = location.href;
      if (/accounts\.google\.com/.test(location.host)) return { state: 'login', url };
      const dialog = dialogText();
      if (promptBox()) return { state: dialog ? 'dialog' : 'ready', url, dialog };
      if (/sign in|ログイン/i.test(text(document.body).slice(0, 2000)) && !turns().length) return { state: 'login', url };
      return { state: 'loading', url, dialog };
    },

    turnCount: () => turns().length,

    snapshot(from) {
      const list = turns();
      const out = [];
      for (let i = from || 0; i < list.length; i++) {
        const t = list[i];
        const role = roleOf(t, i);
        if (role === 'user') {
          out.push({ role, text: toMarkdown(t) });
        } else {
          out.push({
            role,
            text: answerRoots(t).map(toMarkdown).filter(Boolean).join('\n\n'),
            thoughts: thoughtsOf(t),
            sources: sourcesOf(t),
          });
        }
      }
      const rb = runButton();
      return {
        url: location.href,
        generating: isStop(rb),
        turnCount: list.length,
        turns: out,
        errors: errorsNow(),
        dialog: dialogText(),
      };
    },

    focusPrompt() {
      const p = promptBox();
      if (!p) return false;
      p.focus();
      if ('value' in p && p.value) setNativeValue(p, '');
      return true;
    },
    promptValue: () => {
      const p = promptBox();
      return p ? ('value' in p ? p.value : text(p)) : null;
    },
    setPrompt(value) {
      const p = promptBox();
      if (!p) return false;
      p.focus();
      if ('value' in p) setNativeValue(p, value);
      else (p.textContent = value), p.dispatchEvent(new Event('input', { bubbles: true }));
      return true;
    },
    runState() {
      const b = runButton();
      if (!b) return 'missing';
      if (isStop(b)) return 'running';
      return b.disabled || b.getAttribute('aria-disabled') === 'true' ? 'disabled' : 'enabled';
    },
    run() {
      const b = runButton();
      if (!b || isStop(b)) return false;
      click(b);
      return true;
    },
    stop() {
      const b = runButton();
      if (b && isStop(b)) click(b);
      return true;
    },

    async readSettings() {
      await ensureRunSettings();
      await expandSection(LABEL.tools);
      await expandSection(LABEL.advanced);
      const out = { model: currentModel() };
      const thinking = controlNear(LABEL.thinking, SEL.select);
      out.thinking = thinking ? text(thinking).toLowerCase() : null;
      const media = controlNear(LABEL.media, SEL.select);
      out.media_resolution = media ? text(media).toLowerCase() : null;
      for (const key of ['search', 'maps', 'url_context', 'code_execution', 'structured_output', 'function_calling']) {
        const c = controlNear(LABEL[key], SEL.switch);
        out[key] = c ? switchState(c) : null;
      }
      for (const key of ['temperature', 'top_p', 'output_length']) {
        const c = controlNear(LABEL[key], ['input[type="number"]', 'input']);
        out[key] = c && c.value !== '' ? Number(c.value) : null;
      }
      const sys = labelEl(LABEL.system);
      if (sys) {
        const card = sys.parentElement;
        const ta = card && card.querySelector('textarea');
        out.system = ta ? ta.value : null;
      }
      return out;
    },

    async setToggle(key, on) {
      const c = await findControl(key, SEL.switch);
      if (!c) return { ok: false, error: `could not find the "${key}" switch` };
      if (c.disabled || c.getAttribute('aria-disabled') === 'true') {
        return { ok: false, error: `"${key}" is not available for this model` };
      }
      if (switchState(c) !== on) {
        click(c);
        await sleep(250);
      }
      return { ok: switchState(c) === on, error: switchState(c) === on ? null : `"${key}" did not change` };
    },

    async setSelect(key, value) {
      const c = await findControl(key, SEL.select);
      if (!c) return { ok: false, error: `could not find the "${key}" menu` };
      if (text(c).toLowerCase() === value.toLowerCase()) return { ok: true };
      return pickOption(c, value);
    },

    async setNumber(key, value) {
      const c = await findControl(key, ['input[type="number"]', 'input']);
      if (!c) return { ok: false, error: `could not find the "${key}" field` };
      c.focus();
      setNativeValue(c, String(value));
      c.dispatchEvent(new Event('blur', { bubbles: true }));
      return { ok: true };
    },

    async addStop(value) {
      const c = await findControl('stop', ['input']);
      if (!c) return { ok: false, error: 'could not find the stop sequence field' };
      c.focus();
      setNativeValue(c, value);
      pressKey(c, 'Enter');
      return { ok: true };
    },

    async setSystem(value) {
      await ensureRunSettings();
      const label = labelEl(LABEL.system);
      if (!label) return { ok: false, error: 'could not find System instructions' };
      const prompt = promptBox();
      const findBox = () =>
        $all('textarea').find((t) => visible(t) && t !== prompt && /system|instruction|tone|style|指示/i.test((t.getAttribute('aria-label') || '') + ' ' + (t.placeholder || '')))
        || first(SEL.dialog) && $all('textarea', first(SEL.dialog)).find(visible);
      let box = findBox();
      if (!box) {
        click(label.closest('button, [role="button"], mat-expansion-panel-header, [tabindex]') || label);
        await sleep(400);
        box = findBox();
      }
      if (!box) return { ok: false, error: 'could not open System instructions' };
      box.focus();
      setNativeValue(box, value);
      box.dispatchEvent(new Event('blur', { bubbles: true }));
      await sleep(150);
      const dialog = first(SEL.dialog);
      if (dialog) {
        const close = $all('button', dialog).find((b) => /close|done|save|閉じる|完了|保存/i.test(text(b) + ' ' + (b.getAttribute('aria-label') || '')));
        if (close) click(close);
        else pressKey(box, 'Escape');
      }
      return { ok: true };
    },

    currentModel,

    async listModels() {
      const card = modelCard();
      if (!card) return { ok: false, error: 'could not find the model selector', models: [] };
      click(card);
      for (let i = 0; i < 20 && !modelEntries().length; i++) await sleep(150);
      const models = modelEntries().map(({ id, name, description }) => ({ id, name, description }));
      pressKey(document.activeElement || document.body, 'Escape');
      await sleep(200);
      return { ok: models.length > 0, models, error: models.length ? null : 'no models found in the model picker' };
    },

    async selectModel(id) {
      if (currentModel() === id.toLowerCase()) return { ok: true };
      const card = modelCard();
      if (!card) return { ok: false, error: 'could not find the model selector' };
      click(card);
      for (let i = 0; i < 20 && !modelEntries().length; i++) await sleep(150);
      const entry = modelEntries().find((m) => m.id === id.toLowerCase());
      if (!entry) {
        const ids = modelEntries().map((m) => m.id);
        pressKey(document.activeElement || document.body, 'Escape');
        return { ok: false, error: `model ${id} is not offered here (available: ${ids.join(', ') || 'none'})` };
      }
      click(entry.el);
      for (let i = 0; i < 20 && currentModel() !== id.toLowerCase(); i++) await sleep(150);
      if (modelEntries().length) pressKey(document.activeElement || document.body, 'Escape');
      return { ok: currentModel() === id.toLowerCase(), error: currentModel() === id.toLowerCase() ? null : 'the model did not change' };
    },

    // A file input to hand files to (null if the page has none yet).
    fileInput: () => $all('input[type="file"]')[0] || null,

    async openUploadMenu() {
      const b = first(SEL.addMedia);
      if (!b) return { ok: false, error: 'could not find the add-files button' };
      click(b);
      await sleep(300);
      const item = optionsOpen().find((o) => /upload|computer|アップロード/i.test(text(o)));
      if (item) click(item);
      return { ok: true };
    },

    uploadsBusy: () =>
      !!first(['mat-progress-bar', 'mat-progress-spinner', '[role="progressbar"]'], promptBox()?.closest('footer, form, ms-prompt-input-wrapper, ms-prompt-box') || document.body),

    // Compact outline of the page for bug reports (`aistudio debug`).
    outline() {
      const lines = [];
      const walk = (el, depth) => {
        if (depth > 40 || !visible(el)) return;
        const tag = el.tagName.toLowerCase();
        const attrs = ['role', 'aria-label', 'aria-checked', 'aria-expanded', 'placeholder', 'data-turn-role', 'type']
          .map((a) => (el.getAttribute(a) != null ? `${a}="${el.getAttribute(a)}"` : ''))
          .filter(Boolean)
          .join(' ');
        const cls = typeof el.className === 'string' ? el.className.split(/\s+/).filter((c) => c && !c.startsWith('ng-')).slice(0, 4).join('.') : '';
        const own = Array.from(el.childNodes).filter((n) => n.nodeType === 3).map((n) => n.nodeValue.trim()).join(' ').slice(0, 80);
        lines.push(`${'  '.repeat(depth)}<${tag}${cls ? '.' + cls : ''}${attrs ? ' ' + attrs : ''}>${own ? ' ' + JSON.stringify(own) : ''}`);
        for (const c of el.children) walk(c, depth + 1);
      };
      walk(document.body, 0);
      return lines.join('\n');
    },
  };

  window.__aiscli = api;
})();
