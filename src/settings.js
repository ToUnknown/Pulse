const invoke = window.__TAURI__.core.invoke;
const startAtLogin = document.querySelector("#start-at-login");
const trayIcon = document.querySelector("#tray-icon");
const trayIconSection = document.querySelector("#tray-icon-section");
const errorMessage = document.querySelector("#error");

function showError(error) {
  errorMessage.textContent = `Could not save the setting: ${error}`;
  errorMessage.hidden = false;
}

async function loadSettings() {
  try {
    const settings = await invoke("settings_state");
    startAtLogin.checked = settings.startAtLogin;
    if (settings.trayIcon !== null) {
      trayIcon.value = settings.trayIcon;
      trayIconSection.hidden = false;
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

loadSettings();
