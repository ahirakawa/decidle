// decidle Home inside the Herdr app: the rows of `decidle serve`'s /api/sessions, beside the
// terminals. It reads and writes through the HTTP API only; the server keeps the lock and the
// checks that an answer is sent once. See docs/spec-herdr-app.md, "mod".

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, home, readTextFile, run, setBadge, theme, checking } from "herdr";

const DEFAULT_PORT = 7878;
const POLL_MS = 5000;

// The same words as the browser Home (the page table in src/home/server.rs), one table per
// language, picked by `lang` of /api/sessions. `{name}` in a word is filled in by `fill`.
const WORDS = {
  ja: {
    label: { decide: "決めてほしい", permission: "許可待ち", ask: "質問に回答", need: "確認してほしい", unknown: "止まっている", noneed: "対応は不要", working: "作業中", background: "裏で作業中" },
    missing: "一言なしで止まっている",
    unit_sec: "秒",
    unit_min: "分",
    unit_hour: "時間",
    started: "開始 {time}",
    since: "{time}〜",
    failed: "失敗: {status}",
    cannot_send: "送れない: {error}",
    permission: "ターミナルで許可を出してほしい",
    multi: "複数選べる",
    bullet: "・{text}",
    recommend: "推奨: {text}",
    decisions: "決めてほしいこと {n} 件",
    goal: "目的: {text}",
    meta_sep: " ・ ",
    go_ask: "回答しに行く",
    answer: "推奨で進める",
    go: "端末へ行く",
    resolve: "外す",
    open_report: "報告を開く",
    ask_no_term: "この端末は Home から前に出せないので、端末で回答してほしい",
    send_head: "「推奨で進める」で端末に送る文",
    down: "decidle serve が起動していない",
    down_hint: "ターミナルで次を実行する",
    loading: "読み込み中…",
    clear_title: "待ちなし。",
    clear_sub: "そのまま自分の作業を続けて大丈夫。",
    waiting: "待ち",
    background_mark: "(裏)",
    skipped: "読めなかった行: {n} 件 ・ ",
    foot: "5秒ごとに取り直す",
  },
  en: {
    label: { decide: "Decide", permission: "Needs permission", ask: "Answer a question", need: "Please check", unknown: "Stopped", noneed: "Nothing to do", working: "Working", background: "Working in background" },
    missing: "Stopped without a note",
    unit_sec: "s",
    unit_min: "m",
    unit_hour: "h",
    started: "started {time}",
    since: "from {time}",
    failed: "Failed: {status}",
    cannot_send: "Cannot send: {error}",
    permission: "Please grant permission in the terminal",
    multi: "multiple choice",
    bullet: "• {text}",
    recommend: "Recommended: {text}",
    decisions: "{n} decisions to make",
    goal: "Goal: {text}",
    meta_sep: " · ",
    go_ask: "Go answer",
    answer: "Go with recommended",
    go: "Go to terminal",
    resolve: "Dismiss",
    open_report: "Open report",
    ask_no_term: "This terminal cannot be brought forward from Home; answer in the terminal",
    send_head: "What \"Go with recommended\" types into the terminal",
    down: "decidle serve is not running",
    down_hint: "Run this in a terminal",
    loading: "Loading…",
    clear_title: "Nothing waiting.",
    clear_sub: "Carry on with your own work.",
    waiting: "waiting",
    background_mark: " (bg)",
    skipped: "Unreadable lines: {n} · ",
    foot: "Refreshes every 5 s",
  },
};
// Until the server has answered once there is no `lang`; the default of `language` in the config.
const DEFAULT_LANG = "en";
const fill = (word, v) => word.replace(/\{(\w+)\}/g, (_, k) => String(v[k]));
const kind = (s) => (s.state === "stopped" ? { yes: "need", no: "noneed" }[s.attention] || "unknown" : s.state);
const MARK = { decide: "◆", permission: "▲", ask: "▲", need: "●", unknown: "●", noneed: "✓" };
// The buttons are offered on stopped rows only. The server refuses the others with 409.
const canClose = (s) => s.state === "decide" || s.state === "stopped";
const arr = (x) => (Array.isArray(x) ? x : []);
const str = (x) => (typeof x === "string" ? x : "");
// Only a terminal of the Herdr app can be brought forward from inside the app.
const goable = (s) => s.term === "hc" && typeof s.term_id === "string" && s.term_id !== "";

// The start time in this machine's time zone; null for old logs without a start line.
function startTime(s) {
  const t = Date.parse(s.started || "");
  if (isNaN(t)) return null;
  const d = new Date(t), now = new Date(), hm = d.getHours() + ":" + String(d.getMinutes()).padStart(2, "0");
  return d.toDateString() === now.toDateString() ? hm : d.getMonth() + 1 + "/" + d.getDate() + " " + hm;
}

function startLabel(w, s) {
  const time = startTime(s);
  return time === null ? "#" + s.session_id.slice(0, 8) : fill(w.started, { time });
}

function elapsed(w, s) {
  const sec = Math.max(0, Math.floor((Date.now() - Date.parse(s.last_ts)) / 1000)), m = Math.floor(sec / 60);
  if (isNaN(sec)) return "";
  if (m < 1) return sec + w.unit_sec;
  if (m < 60) return m + w.unit_min;
  return Math.floor(m / 60) + ":" + String(m % 60).padStart(2, "0") + w.unit_hour;
}

export default class Decidle extends View {
  init(_props, cx) {
    this.port = DEFAULT_PORT;
    this.data = null; // { sessions, skipped } after the first good read
    this.down = false; // the server did not answer
    this.err = null; // the last refusal of a POST, shown until the next one
    this.sel = null;
    this.token = null;
    this.badge = undefined;
    this.lang = DEFAULT_LANG; // the last `lang` the server sent
    if (checking()) return;
    cx.spawn(async (cx) => {
      this.port = await this.readPort();
      await this.refresh(cx);
    });
    cx.timer.every(POLL_MS, (cx) => {
      cx.spawn(async (cx) => this.refresh(cx));
    });
  }

  // `decidle serve` takes its port from home_port in ~/.config/decidle/config.json.
  async readPort() {
    try {
      const text = await readTextFile(home() + "/.config/decidle/config.json");
      const port = text ? JSON.parse(text).home_port : null;
      return Number.isInteger(port) && port > 0 && port < 65536 ? port : DEFAULT_PORT;
    } catch (_) {
      return DEFAULT_PORT;
    }
  }

  words() {
    return WORDS[this.lang];
  }

  base() {
    return "http://127.0.0.1:" + this.port;
  }

  async refresh(cx) {
    try {
      const res = await fetch(this.base() + "/api/sessions");
      const body = await res.json();
      if (!res.ok || !body || !Array.isArray(body.sessions)) throw new Error(String(res.status));
      this.data = { sessions: body.sessions, skipped: body.skipped || 0 };
      if (WORDS[body.lang]) this.lang = body.lang;
      this.down = false;
    } catch (_) {
      this.data = null;
      this.down = true;
    }
    this.updateBadge();
    cx.notify();
  }

  updateBadge() {
    const n = this.data ? this.data.sessions.filter((s) => s.waits_for_human).length : 0;
    const text = n ? String(n) : null;
    if (text === this.badge) return;
    this.badge = text;
    setBadge(text);
  }

  async readToken() {
    const text = await readTextFile(home() + "/.local/share/decidle/home.token");
    this.token = text ? text.trim() : "";
  }

  // A 403 may mean the server started again with a new token: read the file again, once.
  async post(path, s, cx, payload) {
    this.err = null;
    try {
      if (this.token === null) await this.readToken();
      let res = await this.send(path, s, payload);
      if (res.status === 403) {
        await this.readToken();
        res = await this.send(path, s, payload);
      }
      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        this.err = (body && body.error) || fill(this.words().failed, { status: res.status });
      }
    } catch (e) {
      this.err = fill(this.words().cannot_send, { error: e && e.message ? e.message : e });
    }
    await this.refresh(cx);
  }

  send(path, s, payload) {
    return fetch(this.base() + path, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Decidle-Token": this.token || "", Origin: this.base() },
      body: JSON.stringify(payload || { session_id: s.session_id, resolves: s.last_stop }),
    });
  }

  rows() {
    const all = this.data.sessions.filter((s) => s.state !== "working" && s.state !== "background");
    return [all.filter((s) => s.waits_for_human), all.filter((s) => !s.waits_for_human)];
  }

  name(s) {
    if (this.data.sessions.filter((x) => x.folder === s.folder).length < 2) return s.folder;
    const time = startTime(s);
    return s.folder + " " + (time === null ? startLabel(this.words(), s) : fill(this.words().since, { time }));
  }

  button(t, id, text, primary, onClick) {
    return div()
      .id(id)
      .px(10)
      .py(4)
      .rounded(4)
      .text_size(12)
      .when(primary, (el) => el.bg(t.solid).text_color(t.solid_foreground))
      .when(!primary, (el) => el.border(1).border_color(t.border).text_color(t.foreground).hover((st) => st.bg(t.hover)))
      .child(text)
      .on_click((_e, cx) => {
        cx.stop_propagation();
        onClick(cx);
      });
  }

  say(t, s) {
    const w = this.words();
    if (s.state === "permission") return div().child(w.permission);
    if (s.state === "ask") {
      return v_flex().gap(6).children(
        arr(s.ask).map((q) => q || {}).map((q, i) =>
          v_flex().id("askq-" + i).gap(2)
            .child(h_flex().gap(6)
              .child(div().font_semibold().child(str(q.header)))
              .when(q.multi === true, (el) => el.child(div().text_size(11).text_color(t.muted).child(w.multi))))
            .child(div().child(str(q.question)))
            .child(h_flex().flex_wrap().gap(4).children(
              arr(q.options).map((o, j) => s.choosable === true
                ? this.button(t, "opt-" + i + "-" + j, str(o), false, (cx) => cx.spawn(async (cx) => this.post("/api/choose", s, cx, { session_id: s.session_id, ask: s.ask_ts, option: j })))
                : div().id("opt-" + i + "-" + j).text_size(11).child(fill(w.bullet, { text: str(o) })))))));
    }
    const qs = arr(s.questions), rs = arr(s.recommends);
    if (s.state === "decide" && qs.length) {
      return v_flex().gap(4).children(qs.map((q, i) =>
        v_flex().id("q-" + i)
          .child(div().child(fill(w.bullet, { text: str(q) })))
          .when(!!rs[i], (el) => el.child(div().pl(12).text_color(t.accent).child(fill(w.recommend, { text: str(rs[i]) }))))));
    }
    if (s.state === "decide") return div().child(fill(w.decisions, { n: s.decisions }));
    return div().child(s.note || w.missing);
  }

  // The panel under the selected row: what the agent wants, and what the human can do here.
  sheet(t, s) {
    const w = this.words();
    const close = canClose(s);
    const first = s.answer_text ? "answer" : goable(s) ? "focus" : "resolve";
    const go = () => run("pane.focus", { terminal: s.term_id });
    const meta = [s.report_goal ? fill(w.goal, { text: s.report_goal }) : null, startLabel(w, s), s.agent === "codex" ? "Codex" : null, s.term].filter(Boolean).join(w.meta_sep);
    const buttons = [];
    if (s.state === "ask") {
      if (goable(s)) buttons.push(this.button(t, "go-ask", w.go_ask, true, go));
    } else {
      if (s.answer_text) buttons.push(this.button(t, "answer", w.answer, true, (cx) => cx.spawn(async (cx) => this.post("/api/answer", s, cx))));
      if (close && goable(s)) buttons.push(this.button(t, "go", w.go, first === "focus", go));
      if (close) buttons.push(this.button(t, "resolve", w.resolve, first === "resolve", (cx) => cx.spawn(async (cx) => this.post("/api/resolve", s, cx))));
      if (s.report_url) buttons.push(this.button(t, "report", w.open_report, false, () => { exec("open", [this.base() + s.report_url]); }));
    }
    return v_flex().id("sheet").gap(6).p(10).bg(t.raised).border_b(1).border_color(t.border).text_size(12)
      .child(div().text_color(t.foreground).child(this.say(t, s)))
      .child(div().text_size(11).text_color(t.dim).child(meta))
      .when(s.state === "ask" && !goable(s), (el) => el.child(div().text_size(11).text_color(t.muted).child(w.ask_no_term)))
      .when(!!s.answer_text, (el) => el.child(v_flex().gap(2).p(6).rounded(4).bg(t.surface)
        .child(div().text_size(11).text_color(t.muted).child(w.send_head))
        .child(div().child(str(s.answer_text)))))
      .when(buttons.length > 0, (el) => el.child(h_flex().flex_wrap().gap(6).children(buttons)));
  }

  item(t, s, i) {
    const w = this.words(), k = kind(s), on = s.session_id === this.sel, color = { decide: t.blocked, permission: t.blocked, ask: t.blocked, need: t.done }[k] || t.idle;
    const row = h_flex().id("row-" + s.session_id).gap(8).px(10).py(6).items_center()
      .when(on, (el) => el.bg(t.selected))
      .when(!on, (el) => el.hover((st) => st.bg(t.hover)))
      .on_click((_e, cx) => { this.sel = s.session_id; cx.notify(); })
      .child(div().text_size(11).text_color(t.dim).child(String(i + 1).padStart(2, "0")))
      .child(v_flex().flex_1().min_w(0)
        .child(div().truncate().text_color(s.waits_for_human ? t.foreground : t.muted).child(this.name(s)))
        .child(div().text_size(11).text_color(color).child(MARK[k] + " " + w.label[k])))
      .child(div().text_size(11).text_color(t.dim).child(elapsed(w, s)));
    return on ? [row, this.sheet(t, s)] : [row];
  }

  render() {
    const t = theme(), w = this.words();
    const root = v_flex().id("root").size_full().bg(t.chrome).text_color(t.foreground).text_size(13);
    if (this.down) {
      return root.p(12).gap(6)
        .child(div().font_semibold().child(w.down))
        .child(div().text_color(t.muted).child(w.down_hint))
        .child(div().p(6).rounded(4).bg(t.surface).child("decidle serve"));
    }
    if (!this.data) return root.p(12).child(div().text_color(t.muted).child(w.loading));
    const [wait, done] = this.rows(), all = wait.concat(done);
    if (!all.some((s) => s.session_id === this.sel)) this.sel = all.length ? all[0].session_id : null;
    const busy = this.data.sessions.filter((s) => s.state === "working" || s.state === "background");
    const list = [
      ...wait.flatMap((s, i) => this.item(t, s, i)),
      ...(done.length ? [div().px(10).pt(10).pb(2).text_size(11).text_color(t.dim).child(w.label.noneed), ...done.flatMap((s, i) => this.item(t, s, wait.length + i))] : []),
      ...(all.length ? [] : [v_flex().p(12).gap(2).child(div().font_semibold().child("ALL CLEAR")).child(div().child(w.clear_title)).child(div().text_size(11).text_color(t.muted).child(w.clear_sub))]),
    ];
    return root
      .child(h_flex().px(10).py(8).gap(6).items_center().border_b(1).border_color(t.border)
        .child(div().text_size(18).font_semibold().child(String(wait.length)))
        .child(div().text_color(t.muted).child(w.waiting)))
      .when(!!this.err, (el) => el.child(div().px(10).py(4).text_size(12).text_color(t.blocked).child(this.err)))
      .child(v_flex().id("list").flex_1().overflow_y_scrollbar().children(list))
      .when(busy.length > 0, (el) => el.child(v_flex().px(10).py(6).gap(2).border_t(1).border_color(t.border).text_size(11).text_color(t.muted)
        .child(div().font_semibold().child("NOW WORKING"))
        .children(busy.map((s) => div().id("busy-" + s.session_id).truncate().child("◐ " + this.name(s) + (s.state === "background" ? w.background_mark : "") + " " + elapsed(w, s))))))
      .child(div().px(10).py(4).text_size(11).text_color(t.dim).child((this.data.skipped > 0 ? fill(w.skipped, { n: this.data.skipped }) : "") + w.foot));
  }
}
