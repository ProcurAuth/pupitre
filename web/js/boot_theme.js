// Copied from Hune, web/js/boot_theme.js, commit a55fff3. Edits: the key of the storage is pupitre.theme; the attribute is data-pupitre-hold; the numbers of the Hune paragraphs stay in the comments.
// Classic script (no module). It sets data-theme before the first paint.
// The choice is in localStorage key "pupitre.theme": "system" (default), "light" or "dark".
// The theme control writes the key and then sends the event "pupitre-theme" on document.
// With "system", the theme follows prefers-color-scheme, also when the system changes later.
// data-theme is the resolved theme ("light" or "dark"). data-theme-choice is the choice of the viewer.
// The shell also holds its first paint (attribute data-pupitre-hold, css base.css): main.js removes it when the first view is complete.
// This script removes it after HOLD_MS in all cases, so a page whose main.js does not load or does not end is never hidden.
(function () {
  var root = document.documentElement;
  var HOLD_MS = 2000;

  function stored() {
    try {
      var value = window.localStorage.getItem("pupitre.theme");
      if (value === "light" || value === "dark" || value === "system") return value;
    } catch (error) {
      // Storage can be blocked. The default applies.
    }
    return "system";
  }

  function systemTheme() {
    try {
      return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    } catch (error) {
      return "light";
    }
  }

  function apply() {
    var choice = stored();
    var theme = choice === "system" ? systemTheme() : choice;
    if (root.getAttribute("data-theme-choice") !== choice) root.setAttribute("data-theme-choice", choice);
    if (root.getAttribute("data-theme") === theme) return false;
    root.setAttribute("data-theme", theme);
    return true;
  }

  // A change that the theme control did not make (the system, another tab): the pages that draw on a canvas draw again.
  function follow() {
    if (apply()) document.dispatchEvent(new Event("pupitre-theme"));
  }

  apply();
  window.setTimeout(function () {
    root.removeAttribute("data-pupitre-hold");
  }, HOLD_MS);
  document.addEventListener("pupitre-theme", apply);
  // Another tab changes the choice.
  window.addEventListener("storage", function (event) {
    if (event.key === "pupitre.theme") follow();
  });
  try {
    var media = window.matchMedia("(prefers-color-scheme: dark)");
    if (media.addEventListener) media.addEventListener("change", follow);
  } catch (error) {
    // No media query support: the theme stays as it is.
  }
})();
