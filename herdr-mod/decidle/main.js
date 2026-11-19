// decidle Home inside the Herdr app: the rows of `decidle serve`'s /api/sessions, beside the
// terminals. It reads and writes through the HTTP API only; the server keeps the lock and the
// checks that an answer is sent once. See docs/spec-herdr-app.md, "mod".

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, home, readTextFile, run, setBadge, theme, checking } from "herdr";

const DEFAULT_PORT = 7878;
const POLL_MS = 5000;

// The same words as the browser Home (src/home/page.html).
const kind = (s) => (s.state === "stopped" ? { yes: "need", no: "noneed" }[s.attention] || "unknown" : s.state);
const LABEL = { decide: "決めてほしい", permission: "許可待ち", ask: "質問に回答", need: "確認してほしい", unknown: "止まっている", noneed: "対応は不要", working: "作業中", background: "裏で作業中" };
const MARK = { decide: "◆", permission: "▲", ask: "▲", need: "●", unknown: "●", noneed: "✓" };
const MISSING = "一言なしで止まっている";
// The buttons are offered on stopped rows only. The server refuses the others with 409.
const canClose = (s) => s.state === "decide" || s.state === "stopped";
const arr = (x) => (Array.isArray(x) ? x : []);
const str = (x) => (typeof x === "string" ? x : "");
// Only a terminal of the Herdr app can be brought forward from inside the app.
const goable = (s) => s.term === "hc" && typeof s.term_id === "string" && s.term_id !== "";

function startLabel(s) {
  const t = Date.parse(s.started || "");
  if (isNaN(t)) return "#" + s.session_id.slice(0, 8);
  const d = new Date(t), now = new Date(), hm = d.getHours() + ":" + String(d.getMinutes()).padStart(2, "0");
  return "開始 " + (d.toDateString() === now.toDateString() ? hm : d.getMonth() + 1 + "/" + d.getDate() + " " + hm);
}

function elapsed(s) {
  const sec = Math.max(0, Math.floor((Date.now() - Date.parse(s.last_ts)) / 1000)), m = Math.floor(sec / 60);
  if (isNaN(sec)) return "";
  if (m < 1) return sec + "秒";
  if (m < 60) return m + "分";
  return Math.floor(m / 60) + ":" + String(m % 60).padStart(2, "0") + "時間";
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

  base() {
    return "http://127.0.0.1:" + this.port;
  }

  async refresh(cx) {
    try {
      const res = await fetch(this.base() + "/api/sessions");
      const body = await res.json();
      if (!res.ok || !body || !Array.isArray(body.sessions)) throw new Error(String(res.status));
      this.data = { sessions: body.sessions, skipped: body.skipped || 0 };
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
  async post(path, s, cx) {
    this.err = null;
    try {
      if (this.token === null) await this.readToken();
      let res = await this.send(path, s);
      if (res.status === 403) {
        await this.readToken();
        res = await this.send(path, s);
      }
      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        this.err = (body && body.error) || "失敗: " + res.status;
      }
    } catch (e) {
      this.err = "送れない: " + (e && e.message ? e.message : e);
    }
    await this.refresh(cx);
  }

  send(path, s) {
    return fetch(this.base() + path, {
      method: "POST",
      headers: { "Content-Type": "application/json", "X-Decidle-Token": this.token || "", Origin: this.base() },
      body: JSON.stringify({ session_id: s.session_id, resolves: s.last_stop }),
    });
  }

  rows() {
    const all = this.data.sessions.filter((s) => s.state !== "working" && s.state !== "background");
    return [all.filter((s) => s.waits_for_human), all.filter((s) => !s.waits_for_human)];
  }

  name(s) {
    const same = this.data.sessions.filter((x) => x.folder === s.folder).length > 1;
    return s.folder + (same ? " " + startLabel(s).replace("開始 ", "") + "〜" : "");
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
    if (s.state === "permission") return div().child("ターミナルで許可を出してほしい");
    if (s.state === "ask") {
      return v_flex().gap(6).children(
        arr(s.ask).map((q) => q || {}).map((q, i) =>
          v_flex().id("askq-" + i).gap(2)
            .child(h_flex().gap(6)
              .child(div().font_semibold().child(str(q.header)))
              .when(q.multi === true, (el) => el.child(div().text_size(11).text_color(t.muted).child("複数選べる"))))
            .child(div().child(str(q.question)))
            .child(h_flex().flex_wrap().gap(4).children(
              arr(q.options).map((o, j) => div().id("opt-" + i + "-" + j).px(6).py(1).rounded(3).border(1).border_color(t.border).text_size(11).child(str(o)))))));
    }
    const qs = arr(s.questions), rs = arr(s.recommends);
    if (s.state === "decide" && qs.length) {
      return v_flex().gap(4).children(qs.map((q, i) =>
        v_flex().id("q-" + i)
          .child(div().child("・" + str(q)))
          .when(!!rs[i], (el) => el.child(div().pl(12).text_color(t.accent).child("推奨: " + str(rs[i]))))));
    }
    if (s.state === "decide") return div().child("決めてほしいこと " + s.decisions + " 件");
    return div().child(s.note || MISSING);
  }

  // The panel under the selected row: what the agent wants, and what the human can do here.
  sheet(t, s) {
    const close = canClose(s);
    const first = s.answer_text ? "answer" : goable(s) ? "focus" : "resolve";
    const go = () => run("pane.focus", { terminal: s.term_id });
    const meta = [s.report_goal ? "目的: " + s.report_goal : null, startLabel(s), s.agent === "codex" ? "Codex" : null, s.term].filter(Boolean).join(" ・ ");
    const buttons = [];
    if (s.state === "ask") {
      if (goable(s)) buttons.push(this.button(t, "go-ask", "回答しに行く", true, go));
    } else {
      if (s.answer_text) buttons.push(this.button(t, "answer", "推奨で進める", true, (cx) => cx.spawn(async (cx) => this.post("/api/answer", s, cx))));
      if (close && goable(s)) buttons.push(this.button(t, "go", "端末へ行く", first === "focus", go));
      if (close) buttons.push(this.button(t, "resolve", "外す", first === "resolve", (cx) => cx.spawn(async (cx) => this.post("/api/resolve", s, cx))));
      if (s.report_url) buttons.push(this.button(t, "report", "報告を開く", false, () => { exec("open", [this.base() + s.report_url]); }));
    }
    return v_flex().id("sheet").gap(6).p(10).bg(t.raised).border_b(1).border_color(t.border).text_size(12)
      .child(div().text_color(t.foreground).child(this.say(t, s)))
      .child(div().text_size(11).text_color(t.dim).child(meta))
      .when(s.state === "ask" && !goable(s), (el) => el.child(div().text_size(11).text_color(t.muted).child("この端末は Home から前に出せないので、端末で回答してほしい")))
      .when(!!s.answer_text, (el) => el.child(v_flex().gap(2).p(6).rounded(4).bg(t.surface)
        .child(div().text_size(11).text_color(t.muted).child("「推奨で進める」で端末に送る文"))
        .child(div().child(str(s.answer_text)))))
      .when(buttons.length > 0, (el) => el.child(h_flex().flex_wrap().gap(6).children(buttons)));
  }

  item(t, s, i) {
    const k = kind(s), on = s.session_id === this.sel, color = { decide: t.blocked, permission: t.blocked, ask: t.blocked, need: t.done }[k] || t.idle;
    const row = h_flex().id("row-" + s.session_id).gap(8).px(10).py(6).items_center()
      .when(on, (el) => el.bg(t.selected))
      .when(!on, (el) => el.hover((st) => st.bg(t.hover)))
      .on_click((_e, cx) => { this.sel = s.session_id; cx.notify(); })
      .child(div().text_size(11).text_color(t.dim).child(String(i + 1).padStart(2, "0")))
      .child(v_flex().flex_1().min_w(0)
        .child(div().truncate().text_color(s.waits_for_human ? t.foreground : t.muted).child(this.name(s)))
        .child(div().text_size(11).text_color(color).child(MARK[k] + " " + LABEL[k])))
      .child(div().text_size(11).text_color(t.dim).child(elapsed(s)));
    return on ? [row, this.sheet(t, s)] : [row];
  }

  render() {
    const t = theme();
    const root = v_flex().id("root").size_full().bg(t.chrome).text_color(t.foreground).text_size(13);
    if (this.down) {
      return root.p(12).gap(6)
        .child(div().font_semibold().child("decidle serve が起動していない"))
        .child(div().text_color(t.muted).child("ターミナルで次を実行する"))
        .child(div().p(6).rounded(4).bg(t.surface).child("decidle serve"));
    }
    if (!this.data) return root.p(12).child(div().text_color(t.muted).child("読み込み中…"));
    const [wait, done] = this.rows(), all = wait.concat(done);
    if (!all.some((s) => s.session_id === this.sel)) this.sel = all.length ? all[0].session_id : null;
    const busy = this.data.sessions.filter((s) => s.state === "working" || s.state === "background");
    const list = [
      ...wait.flatMap((s, i) => this.item(t, s, i)),
      ...(done.length ? [div().px(10).pt(10).pb(2).text_size(11).text_color(t.dim).child("対応は不要"), ...done.flatMap((s, i) => this.item(t, s, wait.length + i))] : []),
      ...(all.length ? [] : [v_flex().p(12).gap(2).child(div().font_semibold().child("ALL CLEAR")).child(div().child("待ちなし。")).child(div().text_size(11).text_color(t.muted).child("そのまま自分の作業を続けて大丈夫。"))]),
    ];
    return root
      .child(h_flex().px(10).py(8).gap(6).items_center().border_b(1).border_color(t.border)
        .child(div().text_size(18).font_semibold().child(String(wait.length)))
        .child(div().text_color(t.muted).child("待ち")))
      .when(!!this.err, (el) => el.child(div().px(10).py(4).text_size(12).text_color(t.blocked).child(this.err)))
      .child(v_flex().id("list").flex_1().overflow_y_scrollbar().children(list))
      .when(busy.length > 0, (el) => el.child(v_flex().px(10).py(6).gap(2).border_t(1).border_color(t.border).text_size(11).text_color(t.muted)
        .child(div().font_semibold().child("NOW WORKING"))
        .children(busy.map((s) => div().id("busy-" + s.session_id).truncate().child("◐ " + this.name(s) + (s.state === "background" ? "(裏)" : "") + " " + elapsed(s))))))
      .child(div().px(10).py(4).text_size(11).text_color(t.dim).child((this.data.skipped > 0 ? "読めなかった行: " + this.data.skipped + " 件 ・ " : "") + "5秒ごとに取り直す"));
  }
}
