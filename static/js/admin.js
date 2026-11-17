// Admin UI touches: option rows in the question editor, drag-and-drop reordering,
// print buttons. No inline scripts (CSP).
(function () {
  "use strict";

  function csrfToken() {
    var m = document.querySelector('meta[name="csrf-token"]');
    return m ? m.getAttribute("content") : "";
  }

  // ---- Question editor: add / remove option rows -------------------------
  var rows = document.querySelector("[data-options]");
  var tpl = document.querySelector("[data-option-template]");
  var addBtn = document.querySelector("[data-add-option]");
  var MAX = 8;
  var MIN = 2;

  function rowCount() { return rows ? rows.querySelectorAll("[data-option-row]").length : 0; }
  function nextIndex() {
    var max = -1;
    rows.querySelectorAll("input[name^='opt_text_']").forEach(function (i) {
      var n = parseInt(i.name.replace("opt_text_", ""), 10);
      if (n > max) max = n;
    });
    return max + 1;
  }
  function refresh() {
    if (!rows) return;
    if (addBtn) addBtn.disabled = rowCount() >= MAX;
    rows.querySelectorAll("[data-remove-option]").forEach(function (b) { b.disabled = rowCount() <= MIN; });
  }
  if (rows && tpl && addBtn) {
    addBtn.addEventListener("click", function () {
      if (rowCount() >= MAX) return;
      var html = tpl.innerHTML.replace(/__I__/g, String(nextIndex()));
      var wrap = document.createElement("div");
      wrap.innerHTML = html.trim();
      var row = wrap.firstChild;
      rows.appendChild(row);
      row.querySelector("input.input").focus();
      refresh();
    });
    rows.addEventListener("click", function (e) {
      var btn = e.target.closest("[data-remove-option]");
      if (!btn || rowCount() <= MIN) return;
      btn.closest("[data-option-row]").remove();
      refresh();
    });
    refresh();
  }

  // Single-answer questions: checking one "correct" switch unchecks the others.
  var typeGroup = document.querySelector("[data-answer-type]");
  if (rows && typeGroup) {
    rows.addEventListener("change", function (e) {
      var t = typeGroup.querySelector("input:checked");
      if (!t || t.value !== "single") return;
      if (e.target.name && e.target.name.indexOf("opt_correct_") === 0 && e.target.checked) {
        rows.querySelectorAll("input[name^='opt_correct_']").forEach(function (c) {
          if (c !== e.target) c.checked = false;
        });
      }
    });
  }

  // ---- Drag-and-drop reordering of questions ----------------------------
  var list = document.querySelector("[data-reorder-url]");
  if (list) {
    var dragged = null;
    list.addEventListener("dragstart", function (e) {
      var handle = e.target.closest(".qitem__handle");
      if (!handle) return;
      dragged = handle.closest(".qitem");
      dragged.classList.add("is-dragging");
      e.dataTransfer.effectAllowed = "move";
      e.dataTransfer.setData("text/plain", dragged.dataset.id);
      e.dataTransfer.setDragImage(dragged, 20, 20);
    });
    list.addEventListener("dragover", function (e) {
      if (!dragged) return;
      var over = e.target.closest(".qitem");
      if (!over || over === dragged) return;
      e.preventDefault();
      var rect = over.getBoundingClientRect();
      var after = e.clientY > rect.top + rect.height / 2;
      list.insertBefore(dragged, after ? over.nextSibling : over);
    });
    list.addEventListener("drop", function (e) { if (dragged) e.preventDefault(); });
    list.addEventListener("dragend", function () {
      if (!dragged) return;
      dragged.classList.remove("is-dragging");
      dragged = null;
      var ids = [];
      list.querySelectorAll(".qitem").forEach(function (li, i) {
        ids.push(li.dataset.id);
        var num = li.querySelector(".qitem__num");
        if (num) num.textContent = String(i + 1);
      });
      var body = new URLSearchParams();
      body.set("ids", ids.join(","));
      fetch(list.dataset.reorderUrl, {
        method: "POST",
        headers: { "X-CSRF-Token": csrfToken(), "Content-Type": "application/x-www-form-urlencoded" },
        body: body.toString(),
        credentials: "same-origin"
      }).then(function (r) {
        if (!r.ok) { window.alert("Ordinea nu a putut fi salvată. Pagina va fi reîncărcată."); window.location.reload(); }
      });
    });
  }

  // ---- Live dashboard: pause polling while the tab is hidden --------------
  var live = document.querySelector("[data-live]");
  if (live) {
    // Skip polls while the page is not visible (other tab, minimized window).
    live.addEventListener("htmx:beforeRequest", function (e) {
      if (document.hidden) e.preventDefault();
    });
    // Fresh data immediately when coming back to the tab.
    document.addEventListener("visibilitychange", function () {
      if (!document.hidden) htmx.trigger(live, "refresh");
    });
  }

  // ---- Sortable tables (results) -------------------------------------------
  document.querySelectorAll("table[data-sortable]").forEach(function (table) {
    var buttons = table.querySelectorAll("th .th-sort");
    buttons.forEach(function (btn) {
      btn.addEventListener("click", function () {
        var th = btn.closest("th");
        var col = Array.prototype.indexOf.call(th.parentNode.children, th);
        var numeric = btn.getAttribute("data-sort") === "num";
        var asc = btn.getAttribute("aria-sort") !== "ascending";
        buttons.forEach(function (b) { b.removeAttribute("aria-sort"); });
        btn.setAttribute("aria-sort", asc ? "ascending" : "descending");
        var tbody = table.tBodies[0];
        var rows = Array.prototype.slice.call(tbody.rows);
        var key = function (row) {
          var cell = row.cells[col];
          var v = cell.getAttribute("data-sort-value");
          if (v === null) v = cell.textContent.trim();
          return numeric ? parseFloat(v) : v.toLowerCase();
        };
        rows.sort(function (a, b) {
          var x = key(a), y = key(b);
          var r = numeric ? x - y : x.localeCompare(y, "ro");
          return asc ? r : -r;
        });
        rows.forEach(function (r) { tbody.appendChild(r); });
      });
    });
  });

  // ---- Print buttons -----------------------------------------------------
  document.addEventListener("click", function (e) {
    if (e.target.closest("[data-print]")) window.print();
  });

  // ---- Select-all checkbox in tables -------------------------------------
  document.addEventListener("change", function (e) {
    var all = e.target.closest("[data-select-all]");
    if (!all) return;
    var form = document.getElementById(all.getAttribute("data-select-all"));
    document.querySelectorAll("input[type=checkbox][form='" + form.id + "'][name='ids']").forEach(function (c) { c.checked = all.checked; });
  });
})();
