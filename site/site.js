(() => {
  "use strict";

  const hero = document.querySelector("[data-hero-persona]");
  const heroName = document.querySelector("[data-hero-persona-name]");
  const pickerName = document.querySelector("[data-picker-name]");
  const buttons = [...document.querySelectorAll("[data-persona]")];

  if (!(hero instanceof HTMLImageElement) || !heroName || !pickerName || buttons.length !== 17) {
    return;
  }

  // 中文頁與 en/ 共用這一支：替代文字的句型由各頁的 data-hero-alt 提供。
  const altTemplate = hero.dataset.heroAlt || "{name}";

  for (const button of buttons) {
    button.addEventListener("click", () => {
      const name = button.dataset.name;
      const image = button.querySelector("img");
      if (!name || !(image instanceof HTMLImageElement)) return;

      for (const candidate of buttons) {
        const selected = candidate === button;
        candidate.classList.toggle("selected", selected);
        candidate.setAttribute("aria-pressed", String(selected));
      }

      // 用按鈕裡那張圖已解析好的網址，不自己拼 ./assets/…：en/ 底下的相對路徑不同。
      hero.src = image.src;
      hero.alt = altTemplate.replace("{name}", name);
      heroName.textContent = name;
      pickerName.textContent = name;
    });
  }
})();
