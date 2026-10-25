/* Collapsible groups on the project list (templates/projects.html).
   Each group's toggle shows or hides its projects; which groups are
   collapsed is remembered per browser in localStorage. Storage can be
   missing or refused (private windows, blocked site data): every access is
   wrapped, and the page then just starts with every group open. Without
   this script every group is shown. */
(function () {
  "use strict";
  var KEY = "kestrel.collapsedGroups";

  function load() {
    try {
      var list = JSON.parse(localStorage.getItem(KEY) || "[]");
      return Array.isArray(list) ? list.map(String) : [];
    } catch (e) {
      return [];
    }
  }

  function save(list) {
    try {
      if (list.length) {
        localStorage.setItem(KEY, JSON.stringify(list));
      } else {
        localStorage.removeItem(KEY);
      }
    } catch (e) {
      /* Not remembered; the toggle still works. */
    }
  }

  function apply(button, collapsed) {
    var list = document.getElementById(button.getAttribute("aria-controls"));
    if (list) list.hidden = collapsed;
    button.setAttribute("aria-expanded", collapsed ? "false" : "true");
    button.setAttribute(
      "aria-label",
      button.getAttribute(collapsed ? "data-label-show" : "data-label-hide")
    );
    button.setAttribute(
      "data-tip",
      button.getAttribute(collapsed ? "data-tip-show" : "data-tip-hide")
    );
    var use = button.querySelector("use");
    if (use) {
      use.setAttribute(
        "href",
        "/static/icons.svg#" + (collapsed ? "chevron-down" : "chevron-up")
      );
    }
  }

  function init() {
    var collapsed = load();
    var buttons = document.querySelectorAll("[data-group-toggle]");
    Array.prototype.forEach.call(buttons, function (button) {
      var section = button.closest("[data-project-group]");
      if (!section) return;
      var id = section.getAttribute("data-project-group");
      apply(button, collapsed.indexOf(id) !== -1);
      button.addEventListener("click", function () {
        var now = load();
        var at = now.indexOf(id);
        if (at === -1) {
          now.push(id);
        } else {
          now.splice(at, 1);
        }
        save(now);
        apply(button, at === -1);
      });
    });
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", init);
  } else {
    init();
  }
})();
