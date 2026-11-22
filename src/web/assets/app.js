// Optional enhancement: show timestamps in the visitor's local time zone.
// The page is fully usable without JavaScript.
(function () {
  "use strict";
  var lang = document.documentElement.lang || undefined;
  var fmt;
  try {
    fmt = new Intl.DateTimeFormat(lang, { dateStyle: "medium", timeStyle: "short" });
  } catch (e) {
    return;
  }
  var nodes = document.querySelectorAll("time[data-ts]");
  for (var i = 0; i < nodes.length; i++) {
    var ts = parseInt(nodes[i].getAttribute("data-ts"), 10);
    if (!isNaN(ts)) {
      nodes[i].title = nodes[i].textContent;
      nodes[i].textContent = fmt.format(new Date(ts * 1000));
    }
  }
})();
