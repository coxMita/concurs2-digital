// Load test: N students (default 500) log in within one minute, open the exam,
// answer every question with realistic pauses (autosave), then submit, while the
// teacher watches the live dashboard. Uses only the real UI endpoints.
//
// setup() logs in as admin, creates an exam with 20 questions (bulk import),
// generates N accounts (parsing the one-time credentials page) and opens the exam.
//
// Run through Docker (no local install): ./loadtest/run.sh

import http from "k6/http";
import { check, fail, sleep } from "k6";
import { Counter } from "k6/metrics";
import exec from "k6/execution";

const BASE = __ENV.BASE_URL || "https://localhost";
const STUDENTS = parseInt(__ENV.STUDENTS || "500", 10);
const QUESTIONS = parseInt(__ENV.QUESTIONS || "20", 10);
const LOGIN_WINDOW_S = parseInt(__ENV.LOGIN_WINDOW_S || "60", 10);
const PAUSE_MIN_S = parseFloat(__ENV.PAUSE_MIN_S || "3");
const PAUSE_MAX_S = parseFloat(__ENV.PAUSE_MAX_S || "8");
const ADMIN_USER = __ENV.ADMIN_USER;
const ADMIN_PASS = __ENV.ADMIN_PASS;

const submitted = new Counter("exams_submitted");

export const options = {
  noCookiesReset: true, // the teacher VU stays logged in across iterations
  insecureSkipTLSVerify: true, // Caddy's internal certificate when DOMAIN=localhost
  setupTimeout: "10m",
  scenarios: {
    students: {
      executor: "per-vu-iterations",
      vus: STUDENTS,
      iterations: 1,
      maxDuration: "20m",
      exec: "student",
    },
    teacher: {
      executor: "constant-vus",
      vus: 1,
      duration: `${LOGIN_WINDOW_S + Math.ceil(QUESTIONS * PAUSE_MAX_S) + 30}s`,
      exec: "teacher",
    },
  },
  thresholds: {
    http_req_failed: ["rate==0"],
    "http_req_duration{scenario:students}": ["p(95)<300"],
    "http_req_duration{name:login}": ["p(95)<300"],
    "http_req_duration{name:exam_page}": ["p(95)<300"],
    "http_req_duration{name:autosave}": ["p(95)<300"],
    "http_req_duration{name:submit}": ["p(95)<300"],
    "http_req_duration{name:dashboard}": ["p(95)<300"],
    checks: ["rate==1"],
  },
  summaryTrendStats: ["avg", "med", "p(90)", "p(95)", "p(99)", "max"],
};

function attr(html, name) {
  const m = html.match(new RegExp(`name="${name}" value="([^"]*)"`));
  return m ? m[1] : null;
}
function allAttr(html, name) {
  const re = new RegExp(`name="${name}" value="([^"]*)"`, "g");
  const out = [];
  let m;
  while ((m = re.exec(html)) !== null) out.push(m[1]);
  return out;
}
function meta(html) {
  const m = html.match(/name="csrf-token" content="([^"]*)"/);
  return m ? m[1] : null;
}
function unescape(s) {
  return s.replace(/&amp;/g, "&").replace(/&#x27;|&#39;/g, "'").replace(/&quot;/g, '"').replace(/&lt;/g, "<").replace(/&gt;/g, ">");
}

function adminLogin() {
  const page = http.get(`${BASE}/admin/login`, { tags: { name: "admin" } });
  const res = http.post(
    `${BASE}/admin/login`,
    { _csrf: attr(page.body, "_csrf"), username: ADMIN_USER, password: ADMIN_PASS },
    { tags: { name: "admin" } },
  );
  if (!res.url.includes("/admin/teste")) fail("admin login failed: set ADMIN_USER / ADMIN_PASS");
  return meta(res.body);
}

function questionText() {
  let t = "";
  for (let i = 1; i <= QUESTIONS; i++) {
    if (i % 3 === 0) {
      t += `Î: Întrebarea ${i}: alege toate variantele corecte.\nT: multiplu\n+ Corect A\n+ Corect B\n- Greșit C\n- Greșit D\n\n`;
    } else {
      t += `Î: Întrebarea ${i}: alege varianta corectă.\nP: ${1 + (i % 2)}\n+ Corect\n- Greșit 1\n- Greșit 2\n- Greșit 3\n\n`;
    }
  }
  return t;
}

export function setup() {
  if (!ADMIN_USER || !ADMIN_PASS) fail("ADMIN_USER and ADMIN_PASS are required");
  let csrf = adminLogin();
  const p = { tags: { name: "admin" } };

  const created = http.post(
    `${BASE}/admin/teste/nou`,
    { _csrf: csrf, title: `Test de încărcare ${new Date().toISOString()}`, description: "Test automat (k6).", duration_minutes: "60", result_visibility: "score_and_answers" },
    p,
  );
  const m = created.url.match(/\/admin\/teste\/([0-9a-f-]{36})/);
  if (!m) fail(`could not create exam (status ${created.status})`);
  const exam = m[1];

  const imp = http.post(`${BASE}/admin/teste/${exam}/import`, { _csrf: csrf, text: questionText(), action: "save" }, p);
  check(imp, { "questions imported": (r) => r.body.includes(`Au fost importate ${QUESTIONS}`) });

  const creds = [];
  let remaining = STUDENTS;
  while (remaining > 0) {
    const n = Math.min(remaining, 500);
    const gen = http.post(`${BASE}/admin/teste/${exam}/elevi/genereaza`, { _csrf: csrf, count: String(n), names: "" }, Object.assign({ timeout: "300s" }, p));
    const users = allAttr(gen.body, "u");
    const pws = allAttr(gen.body, "p");
    if (users.length !== n) fail(`expected ${n} accounts, got ${users.length} (status ${gen.status})`);
    for (let i = 0; i < n; i++) creds.push({ u: unescape(users[i]), p: unescape(pws[i]) });
    remaining -= n;
  }

  const open = http.post(`${BASE}/admin/teste/${exam}/stare`, { _csrf: csrf, status: "open" }, p);
  check(open, { "exam opened": (r) => r.body.includes("Testul este deschis") });
  return { exam, creds };
}

function pause() {
  sleep(PAUSE_MIN_S + Math.random() * (PAUSE_MAX_S - PAUSE_MIN_S));
}

export function student(data) {
  // Unique 0..N-1 within the students scenario (__VU is shared with the teacher VU).
  const idx = exec.scenario.iterationInTest;
  const me = data.creds[idx];
  if (!me) return;
  // Everybody logs in within the same minute.
  sleep(Math.random() * LOGIN_WINDOW_S);

  const loginPage = http.get(`${BASE}/`, { tags: { name: "login_page" } });
  const login = http.post(
    `${BASE}/`,
    { _csrf: attr(loginPage.body, "_csrf"), username: me.u, password: me.p },
    { tags: { name: "login" }, redirects: 0 },
  );
  if (!check(login, { "login redirects": (r) => r.status === 303 })) return;

  // Accounts have no name: name prompt first.
  let page = http.get(`${BASE}/nume`, { tags: { name: "name_page" } });
  http.post(`${BASE}/nume`, { _csrf: attr(page.body, "_csrf"), full_name: `Elev Test ${idx + 1}` }, { tags: { name: "name_submit" }, redirects: 0 });

  page = http.get(`${BASE}/test`, { tags: { name: "exam_page" } });
  check(page, { "exam page": (r) => r.status === 200 && r.body.includes("answer-form") });
  const csrf = meta(page.body);
  const total = parseInt((page.body.match(/Întrebarea 1 din (\d+)/) || [0, "0"])[1], 10);
  const hx = { headers: { "X-CSRF-Token": csrf, "HX-Request": "true" }, tags: { name: "autosave" } };

  for (let i = 1; i <= total; i++) {
    if (i > 1) page = http.get(`${BASE}/test?q=${i}`, { tags: { name: "exam_page" } });
    const q = attr(page.body, "q");
    const opts = allAttr(page.body, "opt");
    const multiple = page.body.includes('type="checkbox"');
    pause();
    let chosen;
    if (multiple) {
      chosen = opts.filter(() => Math.random() < 0.5);
      if (chosen.length === 0) chosen = [opts[0]];
    } else {
      chosen = [opts[Math.floor(Math.random() * opts.length)]];
    }
    const body = [`q=${q}`].concat(chosen.map((o) => `opt=${o}`)).join("&");
    const r = http.post(`${BASE}/test/raspuns`, body, Object.assign({}, hx, { headers: Object.assign({ "Content-Type": "application/x-www-form-urlencoded" }, hx.headers) }));
    check(r, { "answer saved": (x) => x.status === 204 });
    // Some students change their mind.
    if (!multiple && Math.random() < 0.2) {
      sleep(1 + Math.random() * 2);
      const other = opts[Math.floor(Math.random() * opts.length)];
      const r2 = http.post(`${BASE}/test/raspuns`, `q=${q}&opt=${other}`, Object.assign({}, hx, { headers: Object.assign({ "Content-Type": "application/x-www-form-urlencoded" }, hx.headers) }));
      check(r2, { "answer changed": (x) => x.status === 204 });
    }
  }

  const confirm = http.get(`${BASE}/test/finalizeaza`, { tags: { name: "confirm_page" } });
  pause();
  const sub = http.post(`${BASE}/test/finalizeaza`, { _csrf: attr(confirm.body, "_csrf") }, { tags: { name: "submit" }, redirects: 0 });
  check(sub, { "submitted": (r) => r.status === 303 });
  const res = http.get(`${BASE}/rezultat`, { tags: { name: "result_page" } });
  if (check(res, { "result page": (r) => r.status === 200 && r.body.includes("Testul a fost trimis") })) submitted.add(1);
}

let teacherReady = false;
export function teacher(data) {
  if (!teacherReady) {
    adminLogin();
    teacherReady = true;
  }
  const r = http.get(`${BASE}/admin/teste/${data.exam}/live/tabel`, { headers: { "HX-Request": "true" }, tags: { name: "dashboard" } });
  check(r, { "dashboard": (x) => x.status === 200 && x.body.includes("stat__value") });
  sleep(5);
}

export function handleSummary(data) {
  const m = data.metrics;
  const line = (name) => {
    const k = `http_req_duration{name:${name}}`;
    const v = m[k] && m[k].values;
    return v ? `${name.padEnd(14)} p95=${v["p(95)"].toFixed(1)}ms  p99=${v["p(99)"].toFixed(1)}ms  max=${v.max.toFixed(1)}ms  n=${m[`http_reqs{name:${name}}`] ? m[`http_reqs{name:${name}}`].values.count : "?"}` : `${name}: no data`;
  };
  const failed = m.http_req_failed ? (m.http_req_failed.values.rate * 100).toFixed(3) : "?";
  const all = m["http_req_duration{scenario:students}"];
  const text = [
    "",
    "==== Concurs load test ====",
    `students: ${STUDENTS}, questions: ${QUESTIONS}`,
    `submitted: ${m.exams_submitted ? m.exams_submitted.values.count : 0}`,
    `error rate: ${failed}%   checks: ${(m.checks.values.rate * 100).toFixed(2)}%`,
    `all student requests p95=${all ? all.values["p(95)"].toFixed(1) : "?"}ms`,
    line("login"),
    line("exam_page"),
    line("autosave"),
    line("submit"),
    line("dashboard"),
    "",
  ].join("\n");
  return {
    stdout: text,
    "/scripts/results/summary.json": JSON.stringify(data, null, 2),
  };
}
