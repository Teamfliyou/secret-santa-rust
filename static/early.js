(function () {
  var d = document.documentElement;
  d.classList.add('js');
  try {
    var reduce = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;
    if (/[?&]anim=1(&|$)/.test(location.search) && !reduce) d.classList.add('will-anim');
  } catch (e) {}
})();
