// Exam pages (questions and the confirm page): countdown (display only; the server
// enforces time), autosave feedback, navigator updates, clearing a single-answer
// selection, and the lockdown (full screen + keyboard lock + hidden content while
// the student is elsewhere).
//
// Moving between questions is an HTMX-boosted swap of <body>, so the document (and
// with it full screen and the keyboard lock) survives navigation. This script is
// loaded once; per-page setup runs again after every boosted swap.
(function () {
  "use strict";

  function examPage() { return !!document.querySelector("main.exam-protected"); }

  // ---- Reporting integrity events to /test/eveniment -----------------------
  // The CSRF token is read on every call: the <meta> lives in <head>, which a
  // boosted swap leaves in place, but read it fresh anyway.
  function report(kind) {
    var meta = document.querySelector('meta[name="csrf-token"]');
    var body = new URLSearchParams();
    body.set("_csrf", meta ? meta.getAttribute("content") : "");
    body.set("kind", kind);
    // sendBeacon still delivers while the page is being hidden or closed.
    if (!(navigator.sendBeacon && navigator.sendBeacon("/test/eveniment", body))) {
      fetch("/test/eveniment", { method: "POST", body: body, credentials: "same-origin", keepalive: true }).catch(function () {});
    }
  }

  // ---- Lockdown: full screen, keyboard lock, shield ------------------------
  // Full screen is required where the browser supports it (not on iPhone). In
  // Chromium browsers (Chrome, Edge, Brave) the keyboard lock then hands the page
  // the keys the system would otherwise take (PrintScreen, Alt+Tab, the Windows
  // key...), so they can be counted and blocked. On Linux the desktop may ask once
  // to allow it. Esc must be held for 2 s to leave full screen, which is counted.
  var canFullscreen = !!(document.fullscreenEnabled && document.documentElement.requestFullscreen);
  function isFull() { return !!document.fullscreenElement; }

  var shield = document.createElement("div");
  shield.className = "exam-shield";
  shield.hidden = true;
  shield.setAttribute("role", "dialog");
  shield.setAttribute("aria-modal", "true");
  shield.innerHTML =
    '<div class="exam-shield__box">' +
    '<h2 class="exam-shield__title"></h2>' +
    '<p class="exam-shield__text"></p>' +
    '<button class="btn btn--primary btn--lg" type="button">Continuă testul</button>' +
    "</div>";
  var shieldTitle = shield.querySelector(".exam-shield__title");
  var shieldText = shield.querySelector(".exam-shield__text");
  var shieldBtn = shield.querySelector("button");
  // On <html>, outside <body>, so boosted swaps leave it alone.
  document.documentElement.appendChild(shield);

  function lockKeys() {
    if (navigator.keyboard && navigator.keyboard.lock) {
      navigator.keyboard.lock().catch(function () {});
    }
  }
  function enterFullscreen() {
    if (!canFullscreen) return;
    var p = document.documentElement.requestFullscreen({ navigationUI: "hide" });
    if (p && p.then) p.then(lockKeys).catch(function () {});
    else lockKeys();
  }
  shieldBtn.addEventListener("click", enterFullscreen);

  function updateShield() {
    var on = examPage();
    var needFull = on && canFullscreen && !isFull();
    var elsewhere = on && (document.hidden || !document.hasFocus());
    if (needFull) {
      shieldTitle.textContent = "Testul se dă pe tot ecranul";
      shieldText.textContent = "Apasă butonul ca să vezi întrebările. Ieșirea din ecranul complet este înregistrată.";
      shieldBtn.hidden = false;
    } else if (elsewhere) {
      shieldTitle.textContent = "Întoarce-te la test";
      shieldText.textContent = "Întrebările sunt ascunse cât timp fereastra testului nu este activă. Dă click aici ca să continui.";
      shieldBtn.hidden = true;
    }
    var show = needFull || elsewhere;
    shield.hidden = !show;
    document.documentElement.classList.toggle("exam-hidden", show);
  }

  // ---- Leaving the page ----------------------------------------------------
  // Leaving = another tab, window or app, or leaving full screen. Moving between
  // questions is not leaving.
  var navigating = false;
  document.addEventListener("click", function (e) {
    var a = e.target.closest && e.target.closest("a[href]");
    if (a && a.origin === window.location.origin) navigating = true;
  }, true);
  document.addEventListener("submit", function () { navigating = true; }, true);
  window.addEventListener("pageshow", function () { navigating = false; });

  var away = false;
  function leave() {
    if (away || navigating || !examPage()) return;
    away = true;
    report("leave");
  }
  function back() {
    if (!away || document.hidden || !document.hasFocus()) return;
    if (canFullscreen && !isFull()) return;
    away = false;
    report("return");
  }
  document.addEventListener("visibilitychange", function () {
    if (document.hidden) leave(); else back();
    updateShield();
  });
  window.addEventListener("blur", function () { leave(); updateShield(); });
  window.addEventListener("focus", function () { back(); updateShield(); });
  document.addEventListener("fullscreenchange", function () {
    if (isFull()) { lockKeys(); back(); } else { leave(); }
    updateShield();
  });

  // ---- Copying, printing, shortcuts --------------------------------------
  function blockCopy(e) {
    if (!examPage()) return;
    var t = e.target;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
    e.preventDefault();
    report("copy");
  }
  document.addEventListener("copy", blockCopy);
  document.addEventListener("cut", blockCopy);
  document.addEventListener("contextmenu", blockCopy);
  document.addEventListener("dragstart", function (e) { if (examPage()) e.preventDefault(); });
  window.addEventListener("beforeprint", function () { if (examPage()) report("copy"); });

  // PrintScreen: without the keyboard lock the system usually swallows the key
  // (Windows 11 Snipping Tool, GNOME, KDE), so the page gets keydown, keyup, both
  // or neither. Listen to both and count one press once.
  function isShotKey(e) {
    return e.key === "PrintScreen" || e.code === "PrintScreen" || e.keyCode === 44;
  }
  var shotDown = false; // keydown already counted; skip its matching keyup
  window.addEventListener("keydown", function (e) {
    if (!examPage()) return;
    if (isShotKey(e)) {
      e.preventDefault();
      if (!e.repeat) { shotDown = true; report("screenshot"); }
      return;
    }
    var k = (e.key || "").toLowerCase();
    if ((e.ctrlKey || e.metaKey) && (k === "c" || k === "x" || k === "p" || k === "s" || k === "u" || k === "a")) {
      e.preventDefault();
      report("copy");
      return;
    }
    // Reaches the page only under the keyboard lock: new tab/window, close tab,
    // switching tabs or apps, the Windows/Super key.
    if (((e.ctrlKey || e.metaKey) && (k === "t" || k === "n" || k === "w" || k === "tab")) ||
        (e.altKey && k === "tab") || k === "meta" || k === "os") {
      e.preventDefault();
    }
  }, true);
  window.addEventListener("keyup", function (e) {
    if (!examPage() || !isShotKey(e)) return;
    e.preventDefault();
    if (shotDown) { shotDown = false; return; }
    report("screenshot");
  }, true);

  // ---- Per-page setup ------------------------------------------------------
  var tickTimer = null;
  function initTimer() {
    clearInterval(tickTimer);
    var timer = document.querySelector("[data-deadline]");
    if (!timer) return;
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
        setTimeout(function () { navigating = true; window.location.href = "/rezultat"; }, 1200);
      }
    };
    tick();
    tickTimer = setInterval(tick, 500);
  }

  function initForm() {
    var form = document.getElementById("answer-form");
    if (!form || form.getAttribute("data-bound")) return;
    form.setAttribute("data-bound", "1");
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
  }

  function initPage(afterSwap) {
    navigating = false;
    if (!examPage()) {
      if (!afterSwap) return;
      // A boosted link ended somewhere else (time ran out, test submitted):
      // load that page normally, which also leaves full screen.
      clearInterval(tickTimer);
      window.location.reload();
      return;
    }
    initTimer();
    initForm();
    updateShield();
  }

  document.addEventListener("htmx:afterSettle", function (e) {
    if (e.detail && e.detail.boosted) initPage(true);
  });
  document.addEventListener("htmx:historyRestore", function () { initPage(true); });
  // A boosted navigation that fails (error page, lost connection): go there normally.
  function fallback(e) {
    if (!e.detail || !e.detail.boosted) return;
    var path = e.detail.pathInfo && e.detail.pathInfo.requestPath;
    navigating = true;
    window.location.href = path || window.location.href;
  }
  document.addEventListener("htmx:responseError", fallback);
  document.addEventListener("htmx:sendError", fallback);

  initPage(false);
})();
