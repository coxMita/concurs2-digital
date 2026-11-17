// Exam page: countdown (display only; the server enforces time), autosave feedback,
// navigator updates, clearing a single-answer selection.
(function () {
  "use strict";

  // ---- Countdown ----------------------------------------------------------
  var timer = document.querySelector("[data-deadline]");
  if (timer) {
    var deadline = Date.parse(timer.getAttribute("data-deadline"));
    var offset = Date.parse(timer.getAttribute("data-now")) - Date.now();
    var out = timer.querySelector("[data-remaining]");
    var done = false;
    var pad = function (n) { return n < 10 ? "0" + n : String(n); };
    var tick = function () {
      var left = Math.max(0, Math.floor((deadline - (Date.now() + offset)) / 1000));
      var h = Math.floor(left / 3600), m = Math.floor((left % 3600) / 60), s = left % 60;
      out.textContent = (h > 0 ? h + ":" + pad(m) : pad(m)) + ":" + pad(s);
      timer.classList.toggle("is-warning", left <= 300 && left > 60);
      timer.classList.toggle("is-critical", left <= 60);
      if (left === 60 || left === 300) timer.setAttribute("aria-live", "assertive");
      if (left <= 0 && !done) {
        done = true;
        // Give the server a moment, then let it finalize and redirect.
        setTimeout(function () { window.location.href = "/rezultat"; }, 1200);
      }
    };
    tick();
    setInterval(tick, 500);
  }

  // ---- Integrity: leaving the page, copying, printing, PrintScreen -------
  // Deterrence and signalling only: the server measures time away, and nothing
  // here can stop a second device. Events go to /test/eveniment.
  var csrfMeta = document.querySelector('meta[name="csrf-token"]');
  var csrf = csrfMeta ? csrfMeta.getAttribute("content") : "";
  function report(kind) {
    var body = new URLSearchParams();
    body.set("_csrf", csrf);
    body.set("kind", kind);
    // sendBeacon still delivers while the page is being hidden or closed.
    if (!(navigator.sendBeacon && navigator.sendBeacon("/test/eveniment", body))) {
      fetch("/test/eveniment", { method: "POST", body: body, credentials: "same-origin", keepalive: true }).catch(function () {});
    }
  }

  // Moving between questions or submitting is not "leaving".
  var navigating = false;
  document.addEventListener("click", function (e) {
    var a = e.target.closest && e.target.closest("a[href]");
    if (a && a.origin === window.location.origin) navigating = true;
  }, true);
  document.addEventListener("submit", function () { navigating = true; }, true);
  window.addEventListener("pageshow", function () { navigating = false; });

  var away = false;
  function leave() {
    if (away || navigating) return;
    away = true;
    report("leave");
  }
  function back() {
    if (!away || document.hidden || !document.hasFocus()) return;
    away = false;
    report("return");
  }
  document.addEventListener("visibilitychange", function () { if (document.hidden) leave(); else back(); });
  window.addEventListener("blur", leave);
  window.addEventListener("focus", back);

  // Block copying the questions (and count attempts).
  function blockCopy(e) {
    var t = e.target;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
    e.preventDefault();
    report("copy");
  }
  document.addEventListener("copy", blockCopy);
  document.addEventListener("cut", blockCopy);
  document.addEventListener("contextmenu", blockCopy);
  document.addEventListener("dragstart", function (e) { e.preventDefault(); });

  document.addEventListener("keydown", function (e) {
    var k = (e.key || "").toLowerCase();
    if ((e.ctrlKey || e.metaKey) && (k === "c" || k === "x" || k === "p" || k === "s" || k === "u" || k === "a")) {
      e.preventDefault();
      report("copy");
    }
  });
  // PrintScreen only reaches the page on keyup (Windows); Win+Shift+S shows up as "leave".
  document.addEventListener("keyup", function (e) {
    if (e.key === "PrintScreen") report("screenshot");
  });
  window.addEventListener("beforeprint", function () { report("copy"); });

  // ---- Autosave feedback -------------------------------------------------
  var form = document.getElementById("answer-form");
  if (!form) return;
  var saved = document.querySelector("[data-saved]");
  var savedText = document.querySelector("[data-saved-text]");
  var hideTimer = null;
  var retried = false;

  function show(ok) {
    if (!saved) return;
    clearTimeout(hideTimer);
    saved.classList.toggle("is-error", !ok);
    savedText.textContent = ok ? "Salvat" : "Nesalvat — verifică conexiunea";
    saved.classList.add("is-visible");
    if (ok) hideTimer = setTimeout(function () { saved.classList.remove("is-visible"); }, 1800);
  }

  function updateNav() {
    var answered = form.querySelectorAll("input[name=opt]:checked").length > 0;
    var pill = document.querySelector(".qnav a[aria-current=step]");
    if (pill) pill.classList.toggle("is-answered", answered);
    var count = document.querySelector("[data-answered]");
    if (count) count.textContent = String(document.querySelectorAll(".qnav a.is-answered").length);
  }

  form.addEventListener("htmx:afterRequest", function (e) {
    if (e.detail.successful) {
      retried = false;
      show(true);
      updateNav();
    } else {
      show(false);
      if (!retried) {
        retried = true;
        setTimeout(function () { htmx.trigger(form, "change"); }, 2500);
      }
    }
  });
  form.addEventListener("htmx:sendError", function () { show(false); });

  var clear = document.querySelector("[data-clear]");
  if (clear) {
    clear.addEventListener("click", function () {
      var any = false;
      form.querySelectorAll("input[name=opt]").forEach(function (i) { if (i.checked) { i.checked = false; any = true; } });
      if (any) htmx.trigger(form, "change");
    });
  }
})();
