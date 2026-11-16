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
