// Appended by the daemon to a slide the deck reader frames (`?deck=1`), never to the file
// (docs/adr/slide-decks.md). In edit mode, a double-click on a text makes it editable in place;
// the edit goes to the app, which asks the daemon to write it. The arrows go to the app.
(function () {
  var INLINE = { B: 1, STRONG: 1, I: 1, EM: 1, BR: 1 };
  var mode = "view";
  var editing = null;
  var before = "";
  var original = "";
  function plain(el) {
    for (var i = 0; i < el.childNodes.length; i++) {
      var n = el.childNodes[i];
      if (n.nodeType === 3 || n.nodeType === 8) continue;
      if (n.nodeType !== 1 || !INLINE[n.tagName] || n.attributes.length > 0 || !plain(n)) return false;
    }
    return true;
  }
  function editable(el) {
    while (el && el !== document.body && el.nodeType === 1) {
      if (!INLINE[el.tagName] && plain(el) && (el.textContent || "").trim()) return el;
      el = el.parentElement;
    }
    return null;
  }
  function text(el) {
    return (el.textContent || "").replace(/\s+/g, " ").trim();
  }
  function overflowing(el) {
    return el.scrollWidth > el.clientWidth + 1 || el.scrollHeight > el.clientHeight + 1;
  }
  var style = document.createElement("style");
  style.textContent =
    "[data-chauffeur-mode=edit] [data-chauffeur-editable]:hover{outline:2px dashed rgba(59,130,246,.7);outline-offset:3px;cursor:text}" +
    "[data-chauffeur-editing]{outline:2px solid rgb(59,130,246)!important;outline-offset:3px}" +
    "[data-chauffeur-overflow]{outline-color:rgb(220,38,38)!important}";
  document.head.appendChild(style);
  function mark() {
    var all = document.body.querySelectorAll("*");
    for (var i = 0; i < all.length; i++) {
      var el = all[i];
      if (!INLINE[el.tagName] && plain(el) && (el.textContent || "").trim()) el.setAttribute("data-chauffeur-editable", "");
    }
  }
  function finish(save) {
    var el = editing;
    if (!el) return;
    editing = null;
    el.removeAttribute("contenteditable");
    el.removeAttribute("data-chauffeur-editing");
    el.removeAttribute("data-chauffeur-overflow");
    if (!save || el.innerHTML === original) {
      el.innerHTML = original;
      parent.postMessage({ type: "chauffeur:editing", editing: false }, "*");
      return;
    }
    var tag = el.tagName.toLowerCase();
    var index = Array.prototype.indexOf.call(document.getElementsByTagName(tag), el);
    parent.postMessage({ type: "chauffeur:edit", tag: tag, index: index, before: before, html: el.innerHTML, overflow: overflowing(el) }, "*");
    parent.postMessage({ type: "chauffeur:editing", editing: false }, "*");
  }
  document.addEventListener("dblclick", function (event) {
    if (mode !== "edit" || editing) return;
    var el = editable(event.target);
    if (!el) return;
    event.preventDefault();
    editing = el;
    before = text(el);
    original = el.innerHTML;
    el.setAttribute("contenteditable", "true");
    el.setAttribute("data-chauffeur-editing", "");
    el.focus();
    parent.postMessage({ type: "chauffeur:editing", editing: true }, "*");
  });
  document.addEventListener("input", function () {
    if (!editing) return;
    if (overflowing(editing)) editing.setAttribute("data-chauffeur-overflow", "");
    else editing.removeAttribute("data-chauffeur-overflow");
  });
  document.addEventListener("paste", function (event) {
    if (!editing) return;
    event.preventDefault();
    var pasted = (event.clipboardData && event.clipboardData.getData("text/plain")) || "";
    document.execCommand("insertText", false, pasted);
  });
  document.addEventListener("focusout", function (event) {
    if (editing && event.target === editing) finish(true);
  });
  document.addEventListener("keydown", function (event) {
    if (editing) {
      if (event.key === "Escape") { event.preventDefault(); finish(false); }
      else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) { event.preventDefault(); finish(true); }
      else if (event.key === "Enter") { event.preventDefault(); document.execCommand("insertLineBreak"); }
      return;
    }
    if (["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " ", "Escape", "c", "e"].indexOf(event.key) >= 0 && !event.metaKey && !event.ctrlKey && !event.altKey) {
      event.preventDefault();
      parent.postMessage({ type: "chauffeur:key", key: event.key }, "*");
    }
  });
  addEventListener("message", function (event) {
    var m = event.data;
    if (!m || typeof m !== "object" || m.type !== "chauffeur:mode") return;
    mode = m.mode === "edit" || m.mode === "comment" ? m.mode : "view";
    document.documentElement.setAttribute("data-chauffeur-mode", mode);
    if (mode !== "edit") finish(true);
  });
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", mark);
  else mark();
})();
