// Small UI touches shared by all pages. No inline scripts (CSP).
(function () {
  "use strict";

  // <form data-confirm="..."> asks for confirmation before submitting.
  document.addEventListener("submit", function (e) {
    var form = e.target;
    var msg = form.getAttribute && form.getAttribute("data-confirm");
    if (msg && !window.confirm(msg)) {
      e.preventDefault();
      e.stopImmediatePropagation();
    }
  }, true);
})();
