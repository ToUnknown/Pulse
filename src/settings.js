const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;
const startAtLogin = document.querySelector("#start-at-login");
const translationEnabled = document.querySelector("#translation-enabled");
const translationLifecycleStatus = document.querySelector("#translation-lifecycle-status");
const translationSection = document.querySelector("#translation-section");
const trayIcon = document.querySelector("#tray-icon");
const trayIconSection = document.querySelector("#tray-icon-section");
const iconHeading = document.querySelector("#icon-heading");
const iconDescription = document.querySelector("#icon-description");
const appearanceSection = document.querySelector("#appearance-section");
const autoLightStart = document.querySelector("#auto-light-start");
const autoDarkStart = document.querySelector("#auto-dark-start");
const apiKeyInput = document.querySelector("#openai-api-key");
const apiKeyMask = document.querySelector("#api-key-mask");
const apiKeyStatus = document.querySelector("#api-key-status");
const saveApiKey = document.querySelector("#save-api-key");
const removeApiKey = document.querySelector("#remove-api-key");
const translationInputDevice = document.querySelector("#translation-input-device");
const errorMessage = document.querySelector("#error");
const customSelects = new Map();
let openCustomSelect = null;
let apiKeyInteractionVersion = 0;

function showError(error) {
  errorMessage.textContent = `Could not save the setting: ${error}`;
  errorMessage.hidden = false;
}

function showApiKeyAttention() {
  apiKeyInput.dataset.attention = "true";
  apiKeyInput.setAttribute("aria-invalid", "true");
}

function clearApiKeyAttention() {
  apiKeyInteractionVersion += 1;
  delete apiKeyInput.dataset.attention;
  apiKeyInput.removeAttribute("aria-invalid");
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
    const triggerRect = trigger.getBoundingClientRect();
    const availableBelow = window.innerHeight - triggerRect.bottom;
    const menuHeight = menu.getBoundingClientRect().height;
    root.classList.toggle(
      "dropdown-up",
      availableBelow < menuHeight + 8 && triggerRect.top > availableBelow,
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

  function rebuildOptions() {
    close();
    optionButtons.length = 0;
    menu.replaceChildren();
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
    refresh();
  }

  rebuildOptions();

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
  if (kind === "time") {
    select.closest(".time-selector")?.addEventListener("click", (event) => {
      if (!root.contains(event.target)) {
        trigger.focus();
        trigger.click();
      }
    });
  }
  root.close = close;
  customSelects.set(select, { refresh, close, rebuildOptions });
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

function configureDeviceSelect(select, devices, selected, emptyLabel) {
  select.replaceChildren();
  const emptyOption = document.createElement("option");
  emptyOption.value = "";
  emptyOption.textContent = emptyLabel;
  select.append(emptyOption);

  for (const device of devices) {
    const option = document.createElement("option");
    option.value = device;
    option.textContent = device;
    select.append(option);
  }

  if (selected && !devices.includes(selected)) {
    const unavailableOption = document.createElement("option");
    unavailableOption.value = selected;
    unavailableOption.textContent = `${selected} (unavailable)`;
    select.append(unavailableOption);
  }

  select.value = selected ?? "";
  if (!select.dataset.enhanced) {
    enhanceSelect(select);
  } else {
    customSelects.get(select)?.rebuildOptions();
  }
  refreshSelect(select);
}

addHourOptions(autoLightStart);
addHourOptions(autoDarkStart);
for (const select of document.querySelectorAll(
  'select[data-custom-select]:not([data-custom-select="audio"])',
)) {
  enhanceSelect(select);
}

document.addEventListener("pointerdown", (event) => {
  if (openCustomSelect && !openCustomSelect.contains(event.target)) {
    openCustomSelect.close();
  }
});

window.addEventListener("blur", () => openCustomSelect?.close());

async function loadSettings() {
  const apiKeyInteractionAtLoad = apiKeyInteractionVersion;
  try {
    const settings = await invoke("settings_state");
    document.documentElement.dataset.platform = settings.platform;
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

    const translation = settings.translation;
    translationEnabled.checked = translation.enabled;
    translationSection.hidden = !translation.enabled;
    configureDeviceSelect(
      translationInputDevice,
      translation.inputDevices ?? [],
      translation.inputDevice,
      "System default",
    );
    const translationActive = translation.status !== "idle";
    setSelectDisabled(translationInputDevice, translationActive);
    apiKeyInput.disabled = translationActive;
    removeApiKey.disabled = translationActive;
    removeApiKey.hidden = !translation.apiKeyConfigured;
    apiKeyStatus.textContent = translation.apiKeyConfigured
      ? settings.platform === "macos"
        ? "Stored in macOS Keychain. Enter a new key to replace it."
        : "Stored in Windows Credential Manager. Enter a new key to replace it."
      : "Required for the OpenAI Live Translation API.";
    apiKeyInput.dataset.configured = String(translation.apiKeyConfigured);
    apiKeyInput.placeholder = translation.apiKeyConfigured ? "" : "sk-…";
    apiKeyMask.hidden = !translation.apiKeyConfigured || apiKeyInput.value.trim() !== "";
    saveApiKey.disabled = translationActive || apiKeyInput.value.trim() === "";
    if (
      settings.apiKeyAttentionRequired &&
      apiKeyInteractionVersion === apiKeyInteractionAtLoad
    ) {
      showApiKeyAttention();
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

translationEnabled.addEventListener("change", async () => {
  const enabled = translationEnabled.checked;
  translationEnabled.disabled = true;
  try {
    const lifecycle = await invoke("set_translation_enabled", { enabled });
    errorMessage.hidden = true;
    await loadSettings();
    translationLifecycleStatus.textContent = lifecycle.restartRequired
      ? `Restart your computer to finish ${enabled ? "adding" : "removing"} Pulse.`
      : "";
    translationLifecycleStatus.hidden = !lifecycle.restartRequired;
  } catch (error) {
    translationEnabled.checked = !enabled;
    showError(error);
    await loadSettings();
  } finally {
    translationEnabled.disabled = false;
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

apiKeyInput.addEventListener("input", () => {
  clearApiKeyAttention();
  apiKeyMask.hidden =
    apiKeyInput.dataset.configured !== "true" || apiKeyInput.value.trim() !== "";
  saveApiKey.disabled = apiKeyInput.disabled || apiKeyInput.value.trim() === "";
});

apiKeyInput.addEventListener("pointerdown", clearApiKeyAttention);
apiKeyInput.addEventListener("keydown", clearApiKeyAttention);

saveApiKey.addEventListener("click", async () => {
  const apiKey = apiKeyInput.value.trim();
  if (!apiKey) {
    return;
  }

  saveApiKey.disabled = true;
  try {
    await invoke("set_openai_api_key", { apiKey });
    apiKeyInput.value = "";
    errorMessage.hidden = true;
    await loadSettings();
  } catch (error) {
    showError(error);
  } finally {
    saveApiKey.disabled = apiKeyInput.disabled || apiKeyInput.value.trim() === "";
  }
});

removeApiKey.addEventListener("click", async () => {
  removeApiKey.disabled = true;
  try {
    await invoke("clear_openai_api_key");
    apiKeyInput.value = "";
    errorMessage.hidden = true;
    await loadSettings();
  } catch (error) {
    showError(error);
  } finally {
    removeApiKey.disabled = false;
  }
});

translationInputDevice.addEventListener("change", async () => {
  try {
    await invoke("set_translation_input_device", {
      name: translationInputDevice.value || null,
    });
    errorMessage.hidden = true;
  } catch (error) {
    showError(error);
    await loadSettings();
  }
});

window.addEventListener("focus", loadSettings);
void listen("api-key-attention-requested", loadSettings);
loadSettings();
