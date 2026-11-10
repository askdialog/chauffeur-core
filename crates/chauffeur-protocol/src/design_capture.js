// Appended by the daemon to a screen served to the app's canvas (story 51), never to the file:
// answers the app, over postMessage, with the element under a point (a comment's pin) and
// whether earlier pins' selectors still match (a stale pin).
(function () {
  function selector(el) {
    var parts = [];
    var cur = el;
    var depth = 0;
    while (cur && cur.nodeType === 1 && cur !== document.body && depth < 6) {
      if (cur.id) {
        parts.unshift("#" + CSS.escape(cur.id));
        break;
      }
      var part = cur.tagName.toLowerCase();
      var classes = Array.prototype.slice.call(cur.classList, 0, 2);
      for (var i = 0; i < classes.length; i++) part += "." + CSS.escape(classes[i]);
      var parent = cur.parentElement;
      if (parent) {
        var same = Array.prototype.filter.call(parent.children, function (c) { return c.tagName === cur.tagName; });
        if (same.length > 1) part += ":nth-of-type(" + (same.indexOf(cur) + 1) + ")";
      }
      parts.unshift(part);
      cur = parent;
      depth++;
    }
    return parts.join(" > ");
  }
  function text(el) {
    var t = (el.innerText || el.textContent || "").replace(/\s+/g, " ").trim();
    return t.length > 80 ? t.slice(0, 77) + "…" : t;
  }
  addEventListener("message", function (event) {
    var m = event.data;
    if (!m || typeof m !== "object" || typeof m.type !== "string" || !event.source) return;
    if (m.type === "chauffeur:element") {
      var el = document.elementFromPoint((innerWidth * m.x) / 100, (innerHeight * m.y) / 100);
      var element = el && el !== document.documentElement && el !== document.body ? { selector: selector(el), text: text(el) } : null;
      event.source.postMessage({ type: "chauffeur:element", request: m.request, element: element }, "*");
    } else if (m.type === "chauffeur:check" && Array.isArray(m.selectors)) {
      var found = {};
      m.selectors.forEach(function (s) {
        try { found[s] = !!document.querySelector(s); } catch (_) { found[s] = false; }
      });
      event.source.postMessage({ type: "chauffeur:check", request: m.request, found: found }, "*");
    }
  });
})();
