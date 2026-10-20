(function () {
  'use strict';
  var reduceMotion = window.matchMedia && matchMedia('(prefers-reduced-motion: reduce)').matches;
  function $(s, r) { return (r || document).querySelector(s); }
  function $$(s, r) { return Array.prototype.slice.call((r || document).querySelectorAll(s)); }
  function rand(a, b) { return a + Math.random() * (b - a); }

  /* ---- Confirmations, dialogues, double envoi ---- */
  $$('form[data-confirm]').forEach(function (f) {
    f.addEventListener('submit', function (e) { if (!window.confirm(f.getAttribute('data-confirm'))) e.preventDefault(); });
  });
  $$('[data-open]').forEach(function (b) {
    b.addEventListener('click', function () {
      var d = document.getElementById(b.getAttribute('data-open'));
      if (d && d.showModal) d.showModal();
    });
  });
  $$('dialog [data-close]').forEach(function (b) {
    b.addEventListener('click', function () { b.closest('dialog').close(); });
  });
  $$('dialog').forEach(function (d) {
    d.addEventListener('click', function (e) { if (e.target === d) d.close(); });
  });
  $$('form[data-once]').forEach(function (f) {
    f.addEventListener('submit', function (e) {
      if (f.getAttribute('data-sent')) { e.preventDefault(); return; }
      f.setAttribute('data-sent', '1');
      var b = $('button[type=submit]', f);
      if (b) setTimeout(function () { b.disabled = true; }, 0);
    });
  });

  /* ---- Import CSV : lit le fichier choisi dans la zone de texte ---- */
  var csvFile = $('[data-csv-file]'), csvText = $('[data-csv-text]');
  if (csvFile && csvText && window.FileReader) {
    csvFile.addEventListener('change', function () {
      var file = csvFile.files && csvFile.files[0];
      if (!file) return;
      var r = new FileReader();
      r.onload = function () { csvText.value = String(r.result || ''); };
      r.readAsText(file);
    });
  }

  /* ---- Copier / imprimer la liste des identifiants ---- */
  $$('[data-copy]').forEach(function (b) {
    b.addEventListener('click', function () {
      var table = document.getElementById(b.getAttribute('data-copy'));
      if (!table) return;
      var text = $$('tr[data-line]', table).map(function (tr) { return tr.getAttribute('data-line'); }).join('\n');
      var done = function () { var old = b.textContent; b.textContent = '✓ Copié !'; setTimeout(function () { b.textContent = old; }, 1800); };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(done, function () {});
      } else {
        var ta = document.createElement('textarea'); ta.value = text; document.body.appendChild(ta); ta.select();
        try { document.execCommand('copy'); done(); } catch (e) {}
        document.body.removeChild(ta);
      }
    });
  });
  $$('[data-print]').forEach(function (b) { b.addEventListener('click', function () { window.print(); }); });

  /* ---- Neige légère ---- */
  if (!reduceMotion) {
    $$('.snow').forEach(function (layer) {
      var n = window.innerWidth < 600 ? 14 : 26;
      for (var i = 0; i < n; i++) {
        var f = document.createElement('i'), s = rand(3, 7);
        f.style.left = rand(0, 100) + '%';
        f.style.width = f.style.height = s + 'px';
        f.style.opacity = String(rand(0.35, 0.9));
        f.style.animationDuration = rand(9, 18) + 's';
        f.style.animationDelay = '-' + rand(0, 18) + 's';
        f.style.setProperty('--sx', rand(-60, 60) + 'px');
        layer.appendChild(f);
      }
    });
  }

  /* ---- Séquence de l'enveloppe ---- */
  var scene = document.getElementById('scene');
  var html = document.documentElement;
  if (!scene || scene.getAttribute('data-anim') !== '1' || !html.classList.contains('will-anim')) return;

  var skip = document.getElementById('skip');
  var timers = [];
  var steps = ['is-drop', 'is-open', 'is-letter', 'is-paper', 'is-name', 'is-details'];
  var finished = false;

  function at(ms, fn) { timers.push(setTimeout(fn, ms)); }
  function set(c) { scene.classList.add(c); }

  function confetti() {
    var box = document.getElementById('confetti');
    if (!box) return;
    var colors = ['#d5b36a', '#e8cf8e', '#b83a3a', '#faf8f4', '#2f7a52'];
    var n = window.innerWidth < 600 ? 26 : 40;
    for (var i = 0; i < n; i++) {
      var p = document.createElement('i'), a = rand(-Math.PI, 0), d = rand(90, 240);
      p.style.setProperty('--dx', Math.cos(a) * d + 'px');
      p.style.setProperty('--dy', Math.sin(a) * d * 0.7 + rand(40, 220) + 'px');
      p.style.setProperty('--rot', rand(-540, 540) + 'deg');
      p.style.setProperty('--c', colors[i % colors.length]);
      p.style.animationDelay = rand(0, 0.35) + 's';
      box.appendChild(p);
    }
    setTimeout(function () { box.innerHTML = ''; }, 3200);
  }

  function finish() {
    if (finished) return;
    finished = true;
    timers.forEach(clearTimeout);
    scene.classList.add('is-skip');
    steps.concat(['anim-run']).forEach(function (c) { scene.classList.remove(c); });
    html.classList.remove('will-anim');
    if (skip) skip.hidden = true;
    window.scrollTo(0, 0);
  }

  if (skip) {
    skip.hidden = false;
    skip.addEventListener('click', finish);
  }
  document.addEventListener('keydown', function (e) { if (e.key === 'Escape') finish(); });

  scene.classList.add('anim-run');
  at(400, function () { set('is-drop'); });
  at(3500, function () { set('is-open'); });
  at(4600, function () { set('is-letter'); });
  at(6300, function () { set('is-paper'); window.scrollTo(0, 0); });
  at(7300, function () { set('is-name'); confetti(); });
  at(9200, function () { set('is-details'); });
  at(12500, function () { if (skip) skip.hidden = true; });
})();
