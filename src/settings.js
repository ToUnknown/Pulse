const invoke = window.__TAURI__.core.invoke;
const startAtLogin = document.querySelector("#start-at-login");
const trayIcon = document.querySelector("#tray-icon");
const trayIconSection = document.querySelector("#tray-icon-section");
const iconHeading = document.querySelector("#icon-heading");
const iconDescription = document.querySelector("#icon-description");
const appearanceSection = document.querySelector("#appearance-section");
const autoLightStart = document.querySelector("#auto-light-start");
const autoDarkStart = document.querySelector("#auto-dark-start");
const errorMessage = document.querySelector("#error");
const customSelects = new Map();
let openCustomSelect = null;

function showError(error) {
  errorMessage.textContent = `Could not save the setting: ${error}`;
  errorMessage.hidden = false;
}

function addHourOptions(select) {
  for (let hour = 0; hour < 24; hour += 1) {
    const option = document.createElement("option");
    option.value = String(hour);
    option.textContent = `${String(hour).padStart(2, "0")}:00`;
    select.append(option);
  }
}

function addTrayGlyph(element, value) {
  const glyph = document.createElement("span");
  glyph.className = `tray-glyph${value === "red" ? " tray-glyph-red" : ""}`;
  glyph.setAttribute("aria-hidden", "true");
  element.append(glyph);
}

function enhanceSelect(select) {
  const root = document.createElement("div");
  const trigger = document.createElement("button");
  const menu = document.createElement("div");
  const optionButtons = [];
  const kind = select.dataset.customSelect;

  root.className = `custom-select custom-select--${kind}`;
  trigger.className = "custom-select-trigger";
  trigger.type = "button";
  trigger.setAttribute("aria-expanded", "false");
  trigger.setAttribute("aria-haspopup", "listbox");
  trigger.setAttribute("aria-controls", `${select.id}-menu`);
  trigger.setAttribute("aria-label", select.getAttribute("aria-label"));

  menu.id = `${select.id}-menu`;
  menu.className = "custom-select-menu";
  menu.role = "listbox";
  menu.hidden = true;

  function fillOption(element, option) {
    element.replaceChildren();
    if (kind === "icon") {
      addTrayGlyph(element, option.value);
    }
    const text = document.createElement("span");
    text.textContent = option.textContent;
    element.append(text);
  }

  function refresh() {
    const selectedOption = select.selectedOptions[0];
    if (!selectedOption) {
      return;
    }
    fillOption(trigger, selectedOption);
    trigger.disabled = select.disabled;
    for (const button of optionButtons) {
      const isSelected = button.dataset.value === select.value;
      button.setAttribute("aria-selected", String(isSelected));
      button.tabIndex = isSelected ? 0 : -1;
    }
  }

  function close({ focusTrigger = false } = {}) {
    root.dataset.open = "false";
    root.classList.remove("dropdown-up");
    trigger.setAttribute("aria-expanded", "false");
    menu.hidden = true;
    if (openCustomSelect === root) {
      openCustomSelect = null;
    }
    if (focusTrigger) {
      trigger.focus();
    }
  }

  function open({ focusOption = false } = {}) {
    if (trigger.disabled) {
      return;
    }
    if (openCustomSelect && openCustomSelect !== root) {
      openCustomSelect.close();
    }
    root.dataset.open = "true";
    trigger.setAttribute("aria-expanded", "true");
    menu.hidden = false;
    const availableBelow = window.innerHeight - trigger.getBoundingClientRect().bottom;
    root.classList.toggle(
      "dropdown-up",
      availableBelow < Math.min(menu.scrollHeight, 208) + 8 &&
        trigger.getBoundingClientRect().top > availableBelow,
    );
    openCustomSelect = root;
    const selectedButton = optionButtons.find((button) => button.dataset.value === select.value);
    selectedButton?.scrollIntoView({ block: "nearest" });
    if (focusOption) {
      selectedButton?.focus();
    }
  }

  function choose(button) {
    const changed = select.value !== button.dataset.value;
    select.value = button.dataset.value;
    refresh();
    close({ focusTrigger: true });
    if (changed) {
      select.dispatchEvent(new Event("change", { bubbles: true }));
    }
  }

  for (const option of select.options) {
    const button = document.createElement("button");
    button.className = "custom-select-option";
    button.type = "button";
    button.role = "option";
    button.dataset.value = option.value;
    fillOption(button, option);
    button.addEventListener("click", () => choose(button));
    button.addEventListener("keydown", (event) => {
      const currentIndex = optionButtons.indexOf(button);
      let nextIndex = null;
      if (event.key === "ArrowDown") {
        nextIndex = Math.min(currentIndex + 1, optionButtons.length - 1);
      }
      if (event.key === "ArrowUp") nextIndex = Math.max(currentIndex - 1, 0);
      if (event.key === "Home") nextIndex = 0;
      if (event.key === "End") nextIndex = optionButtons.length - 1;
      if (nextIndex !== null) {
        event.preventDefault();
        optionButtons[nextIndex].focus();
      } else if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        choose(button);
      } else if (event.key === "Escape") {
        event.preventDefault();
        close({ focusTrigger: true });
      } else if (event.key === "Tab") {
        close();
      }
    });
    optionButtons.push(button);
    menu.append(button);
  }

  trigger.addEventListener("click", () => {
    if (menu.hidden) {
      open();
    } else {
      close();
    }
  });
  trigger.addEventListener("keydown", (event) => {
    if (["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) {
      event.preventDefault();
      open({ focusOption: true });
    }
  });

  select.classList.add("custom-select-source");
  select.dataset.enhanced = "true";
  select.tabIndex = -1;
  select.setAttribute("aria-hidden", "true");
  select.after(root);
  root.append(trigger, menu);
  root.close = close;
  customSelects.set(select, { refresh, close });
  new MutationObserver(refresh).observe(select, {
    attributes: true,
    attributeFilter: ["disabled"],
  });
  refresh();
}

function refreshSelect(select) {
  customSelects.get(select)?.refresh();
}

function setSelectDisabled(select, disabled) {
  select.disabled = disabled;
  refreshSelect(select);
}

addHourOptions(autoLightStart);
addHourOptions(autoDarkStart);
for (const select of document.querySelectorAll("select[data-custom-select]")) {
  enhanceSelect(select);
}

document.addEventListener("pointerdown", (event) => {
  if (openCustomSelect && !openCustomSelect.contains(event.target)) {
    openCustomSelect.close();
  }
});

window.addEventListener("blur", () => openCustomSelect?.close());

async function loadSettings() {
  try {
    const settings = await invoke("settings_state");
    startAtLogin.checked = settings.startAtLogin;
    if (settings.platform === "macos") {
      iconHeading.textContent = "Menu-bar icon";
      iconDescription.textContent = "Choose how Pulse appears in the menu bar.";
    }
    if (settings.trayIcon !== null) {
      trayIcon.value = settings.trayIcon;
      refreshSelect(trayIcon);
      trayIconSection.hidden = false;
    }
    if (settings.autoSchedule !== null) {
      autoLightStart.value = String(settings.autoSchedule.lightStart);
      autoDarkStart.value = String(settings.autoSchedule.darkStart);
      refreshSelect(autoLightStart);
      refreshSelect(autoDarkStart);
      appearanceSection.hidden = false;
    }
  } catch (error) {
    showError(error);
  }
}

startAtLogin.addEventListener("change", async () => {
  try {
    await invoke("set_start_at_login", { enabled: startAtLogin.checked });
    errorMessage.hidden = true;
  } catch (error) {
    startAtLogin.checked = !startAtLogin.checked;
    showError(error);
  }
});

trayIcon.addEventListener("change", async () => {
  try {
    await invoke("set_tray_icon_mode", { mode: trayIcon.value });
    errorMessage.hidden = true;
  } catch (error) {
    showError(error);
    await loadSettings();
  }
});

async function saveAutoSchedule() {
  const lightStart = Number(autoLightStart.value);
  const darkStart = Number(autoDarkStart.value);
  if (lightStart === darkStart) {
    showError("choose different start times for light and dark mode");
    return;
  }

  setSelectDisabled(autoLightStart, true);
  setSelectDisabled(autoDarkStart, true);
  try {
    await invoke("set_auto_schedule", { lightStart, darkStart });
    errorMessage.hidden = true;
  } catch (error) {
    showError(error);
    await loadSettings();
  } finally {
    setSelectDisabled(autoLightStart, false);
    setSelectDisabled(autoDarkStart, false);
  }
}

autoLightStart.addEventListener("change", saveAutoSchedule);
autoDarkStart.addEventListener("change", saveAutoSchedule);

loadSettings();
