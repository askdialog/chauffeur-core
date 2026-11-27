// Appended by the daemon to an HTML document the deliverable reader frames (`?fit=1`), never to
// the file. The app says how to fit it ("page" or "width") and how much the reader zoomed on top;
// the page scales itself with a transform, which leaves its layout (and `vw`, `vh`) as the frame's
// size made it. A pinch, Ctrl or ⌘ with the wheel, and ⌘+ ⌘- ⌘0 zoom from inside the frame; the
// page tells the app the zoom and the scale it ends up at.
(function () {
  var mode = "width";
  var zoom = 1;
  var root = document.documentElement;
  var MIN = 0.1;
  var MAX = 5;
  var scale = 1;
  var left = 0;
  function natural() {
    var transform = root.style.transform;
    root.style.transform = "";
    var body = document.body;
    var size = {
      width: Math.max(root.scrollWidth, body ? body.scrollWidth : 0),
      height: Math.max(root.scrollHeight, body ? body.scrollHeight : 0),
    };
    root.style.transform = transform;
    return size;
  }
  function fitted(size) {
    var across = Math.min(1, window.innerWidth / size.width);
    var down = Math.min(1, window.innerHeight / size.height);
    // A page more than twice too long would turn unreadable: it fits the width and scrolls.
    return mode === "page" && down >= across / 2 ? Math.min(across, down) : across;
  }
  // Scales the page; `at`, a point of the frame, keeps what lies under it.
  function apply(at) {
    var size = natural();
    var before = at && { x: (at.x + window.scrollX - left) / scale, y: (at.y + window.scrollY) / scale };
    scale = Math.min(MAX, Math.max(MIN, fitted(size) * zoom));
    // Centred when it ends up narrower than the frame.
    left = Math.max(0, (window.innerWidth - size.width * scale) / 2);
    if (Math.abs(scale - 1) < 0.001 && left === 0) {
      root.style.transform = "";
      root.style.transformOrigin = "";
    } else {
      root.style.transformOrigin = "0 0";
      root.style.transform = "translateX(" + left + "px) scale(" + scale + ")";
    }
    if (before) window.scrollTo(before.x * scale + left - at.x, before.y * scale - at.y);
    window.parent.postMessage({ type: "chauffeur:zoomed", zoom: zoom, scale: scale }, "*");
  }
  function zoomBy(factor, at) {
    var fit = fitted(natural());
    zoom = Math.min(MAX / fit, Math.max(MIN / fit, zoom * factor));
    apply(at);
  }
  window.addEventListener("message", function (event) {
    var data = event.data;
    if (!data || data.type !== "chauffeur:fit" || event.source !== window.parent) return;
    mode = data.mode === "page" ? "page" : "width";
    if (typeof data.zoom === "number" && data.zoom > 0) zoom = data.zoom;
    apply();
  });
  // A trackpad's pinch reaches Chrome as a wheel with Ctrl.
  window.addEventListener("wheel", function (event) {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    // A pinch sends small steps, a mouse's wheel large ones: a notch zooms by about a fifth.
    var step = Math.max(-50, Math.min(50, event.deltaY));
    zoomBy(Math.exp(-step / 250), { x: event.clientX, y: event.clientY });
  }, { passive: false });
  // And WebKit's, the app's window, as gestures.
  var pinched = 1;
  window.addEventListener("gesturestart", function (event) { event.preventDefault(); pinched = 1; });
  window.addEventListener("gesturechange", function (event) {
    event.preventDefault();
    zoomBy(event.scale / pinched, { x: event.clientX, y: event.clientY });
    pinched = event.scale;
  });
  window.addEventListener("gestureend", function (event) { event.preventDefault(); });
  window.addEventListener("keydown", function (event) {
    if (!event.ctrlKey && !event.metaKey) return;
    if (event.key === "=" || event.key === "+") zoomBy(1.25);
    else if (event.key === "-") zoomBy(1 / 1.25);
    else if (event.key === "0") { zoom = 1; apply(); }
    else return;
    event.preventDefault();
  });
  window.addEventListener("resize", function () { apply(); });
  window.addEventListener("load", function () { apply(); });
  if (window.ResizeObserver) {
    var observer = new ResizeObserver(function () { apply(); });
    var watch = function () { if (document.body) observer.observe(document.body); };
    if (document.body) watch(); else document.addEventListener("DOMContentLoaded", watch);
  }
})();
