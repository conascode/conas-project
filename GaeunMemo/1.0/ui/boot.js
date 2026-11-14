// Runs synchronously in <head> so the first paint already has the right theme and band.
(function () {
  var q = new URLSearchParams(location.search);
  var root = document.documentElement;
  root.dataset.theme = q.get('t') === 'light' ? 'light' : 'dark';
  var b = q.get('b');
  root.dataset.band = (b === 'none' || b === 'full') ? b : 'thin';
})();
