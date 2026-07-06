const LANG_KEY = "solopic.site.lang";

function detectLang() {
  const saved = localStorage.getItem(LANG_KEY);
  if (saved && I18N[saved]) return saved;
  const nav = (navigator.language || "en").toLowerCase();
  for (const code of Object.keys(I18N)) {
    if (nav.startsWith(code)) return code;
  }
  return "en";
}

let LANG = detectLang();

function apply() {
  document.documentElement.lang = LANG;
  const d = I18N[LANG] || I18N.en;
  document.title = "SoloPic — " + d.tagline;
  document.querySelectorAll("[data-i18n]").forEach((el) => {
    el.textContent = (I18N[LANG] && I18N[LANG][el.dataset.i18n]) ?? I18N.en[el.dataset.i18n] ?? "";
  });
}

const sel = document.getElementById("lang-sel");
sel.value = LANG;
sel.addEventListener("change", () => {
  LANG = sel.value;
  localStorage.setItem(LANG_KEY, LANG);
  apply();
});

apply();
