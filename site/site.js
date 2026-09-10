(() => {
  "use strict";

  const hero = document.querySelector("[data-hero-persona]");
  const heroName = document.querySelector("[data-hero-persona-name]");
  const pickerName = document.querySelector("[data-picker-name]");
  const buttons = [...document.querySelectorAll("[data-persona]")];

  if (!(hero instanceof HTMLImageElement) || !heroName || !pickerName || buttons.length !== 17) {
    return;
  }

  for (const button of buttons) {
    button.addEventListener("click", () => {
      const id = button.dataset.persona;
      const name = button.dataset.name;
      if (!id || !name) return;

      for (const candidate of buttons) {
        const selected = candidate === button;
        candidate.classList.toggle("selected", selected);
        candidate.setAttribute("aria-pressed", String(selected));
      }

      hero.src = `./assets/personas/${id}.webp`;
      hero.alt = `${name} 角色全身圖`;
      heroName.textContent = name;
      pickerName.textContent = name;
    });
  }
})();
