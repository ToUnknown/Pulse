(() => {
  const query = new URLSearchParams(location.search);
  const scenario = query.get('scenario') || 'success';
  const text = 'A little space to think.\n\nGood ideas often begin with something small: a line in a book, a passing thought, a few words worth keeping.\n\nMake room for what matters.';
  const platform = query.get('platform') || 'windows';
  let settings = { enabled: query.has('enabled'), editorMode: query.get('editorMode') || 'basic', quickMode: query.get('quickMode') || 'basic', shortcut: 'Super+Shift+KeyT', quickShortcut: 'Control+Super+Shift+KeyT', apiKeyConfigured: query.has('key'), error: null, localOcr: { phase: 'ready' } };
  settings.advancedProvider = query.get('provider') || 'codex';
  settings.codex = { installed: query.has('codex'), available: query.has('codex') && scenario !== 'codex-signed-out', checking: false,
    message: scenario === 'codex-signed-out' ? 'Sign in to Codex with ChatGPT, then Retry.' : 'Uses your Codex plan for Advanced and Translate.' };
  function access() {
    const activeAdvancedProvider = settings.codex.installed && settings.advancedProvider === 'codex' ? 'codex' : 'api';
    return { advancedProvider: settings.advancedProvider, activeAdvancedProvider, codex: { ...settings.codex },
      advancedAvailable: activeAdvancedProvider === 'codex' ? settings.codex.available : settings.apiKeyConfigured };
  }
  settings.captureAccess = { supported: true, granted: scenario !== 'capture-denied' };
  if (scenario === 'ocr-preparing') settings.localOcr = { phase: 'preparing' };
  if (scenario === 'ocr-error') settings.localOcr = { phase: 'error', error: 'Could not prepare on-device OCR. Try again.' };
  if (platform === 'macos') {
    settings.shortcut = 'Alt+Shift+KeyT';
    settings.quickShortcut = 'Control+Alt+Shift+KeyT';
  }
  window.__PULSE_FLOATING_SETTINGS__ = true;
  const calls = [];
  window.__preview = { calls, copied: null, closed: false };
  const screenshot = () => {
    const width = 1440, height = 900;
    const canvas = document.createElement('canvas');
    canvas.width = width; canvas.height = height;
    const ctx = canvas.getContext('2d');
    const gradient = ctx.createLinearGradient(0, 0, width, height);
    gradient.addColorStop(0, '#497b9b'); gradient.addColorStop(.5, '#526a9b'); gradient.addColorStop(1, '#b298a3');
    ctx.fillStyle = gradient; ctx.fillRect(0, 0, width, height);
    ctx.fillStyle = '#f4f3ef'; ctx.beginPath(); ctx.roundRect(180, 115, 1080, 650, 14); ctx.fill();
    ctx.fillStyle = '#e7e7e3'; ctx.fillRect(180, 165, 210, 595);
    ctx.fillStyle = '#657174'; ctx.font = '14px system-ui'; ctx.fillText('Field notes', 210, 148);
    ctx.font = '13px system-ui'; for (const [i, label] of ['Quick notes', 'Ideas & inspiration', 'Reading list', 'Archive'].entries()) ctx.fillText(label, 210, 215 + i * 42);
    ctx.fillStyle = '#959b97'; ctx.font = '12px system-ui'; ctx.fillText('PERSONAL / NOTES', 475, 241);
    ctx.fillStyle = '#252f2c'; ctx.font = '600 33px Georgia'; ctx.fillText('A little space to think.', 475, 314);
    ctx.font = '19px Georgia';
    for (const [i, line] of ['Good ideas often begin with something small: a line in a book,', 'a passing thought, a few words worth keeping.', '', 'Make room for what matters.'].entries()) ctx.fillText(line, 475, 372 + i * 35);
    ctx.fillStyle = '#35445cbb'; ctx.fillRect(0, 851, width, 49);
    ctx.fillStyle = '#e2e8f2'; ctx.font = '14px system-ui'; ctx.fillText('⊞     ⌕     ▣     ◉     ✉', 610, 882); ctx.fillText(new Date().toLocaleTimeString('en-GB'), 1340, 876);
    ctx.fillStyle = '#818da044'; ctx.fillRect(475, 650, 220, 4);
    ctx.fillStyle = '#818da0'; ctx.fillRect(475, 650, (Date.now() / 60) % 220, 4);
    return canvas;
  };
  let desktop;
  document.addEventListener('DOMContentLoaded', () => {
    desktop = screenshot();
    desktop.id = 'preview-desktop';
    Object.assign(desktop.style, { position: 'fixed', inset: '0', width: '100%', height: '100%', zIndex: '-1', pointerEvents: 'none' });
    if (location.pathname !== '/settings.html') document.body.prepend(desktop);
    setInterval(() => desktop.getContext('2d').drawImage(screenshot(), 0, 0), 100);
  });
  window.__TAURI__ = { core: { invoke: async (command, args = {}) => {
    calls.push({ command, args: command === 'save_openai_api_key' ? '[redacted fixture]' : args });
    switch (command) {
      case 'settings_state': return { platform, startAtLogin: false, trayIcon: 'default', autoSchedule: platform === 'macos' ? null : { lightStart: 7, darkStart: 19 } };
      case 'text_extractor_state': return { ...settings, ...access() };
      case 'text_extractor_advanced_access': return access();
      case 'set_text_extractor_provider': settings.advancedProvider = args.provider; return;
      case 'refresh_text_extractor_codex': settings.codex.available = true; settings.codex.message = 'Uses your Codex plan for Advanced and Translate.'; return;
      case 'request_text_extractor_access': settings.captureAccess.granted = true; return;
      case 'local_ocr_state': return settings.localOcr;
      case 'retry_local_ocr_setup': settings.localOcr = { phase: 'ready' }; return;
      case 'save_openai_api_key':
        await new Promise(resolve => setTimeout(resolve, scenario === 'key-pending' ? 2500 : 120));
        if (scenario === 'key-invalid') throw 'Not a valid OpenAI API key.';
        if (scenario === 'key-offline') throw 'Could not reach OpenAI. Check your connection and try again.';
        if (scenario === 'key-error') throw 'Could not save the OpenAI key securely.';
        settings.apiKeyConfigured = true; return;
      case 'clear_openai_api_key': if (scenario === 'key-error') throw 'Could not remove the shared OpenAI key from credential storage.'; settings.apiKeyConfigured = false; return;
      case 'set_text_extractor': {
        const normalize = value => value.toLowerCase().split('+').map(part => part.replace(/^key/, '')).sort().join('+');
        if (normalize(args.shortcutValue) === normalize(args.quickShortcutValue)) throw 'Choose different shortcuts for Quick copy and Open editor.';
        if (scenario === 'shortcut-error') throw 'This shortcut is already in use. Choose another combination.';
        settings = { ...settings, enabled: args.enabled, shortcut: args.shortcutValue, quickShortcut: args.quickShortcutValue, editorMode: args.editorMode, quickMode: args.quickMode }; return;
      }
      case 'settings_window_action': if (args.action === 'done') window.__preview.closed = true; return;
      case 'text_extractor_ready': return !query.has('warm');
      case 'text_extractor_capture': return { width: 1440, height: 900, mode: query.has('quick') ? 'quick' : 'editor', defaultMode: query.has('quick') ? settings.quickMode : settings.editorMode };
      case 'text_extractor_capture_selection': {
        const source = desktop || screenshot();
        const canvas = document.createElement('canvas'); canvas.width = args.crop.width; canvas.height = args.crop.height;
        canvas.getContext('2d').drawImage(source, args.crop.x, args.crop.y, args.crop.width, args.crop.height, 0, 0, args.crop.width, args.crop.height);
        const payload = { imageUrl: canvas.toDataURL('image/png') };
        await new Promise(resolve => setTimeout(resolve, scenario === 'capture-pending' ? 3000 : 60));
        if (scenario === 'capture-error') throw 'Windows could not capture this selection.';
        return payload;
      }
      case 'text_extractor_backdrop': {
        const url = (desktop || screenshot()).toDataURL('image/png');
        await new Promise(resolve => setTimeout(resolve, scenario === 'backdrop-pending' ? 3000 : 100));
        if (scenario === 'backdrop-error') throw 'Could not prepare the backdrop.';
        return url;
      }
      case 'text_extractor_quick_copy': {
        await new Promise(resolve => setTimeout(resolve, 100));
        window.__preview.closed = true;
        if (scenario === 'basic-error' || scenario === 'clipboard-error') { window.__preview.error = 'Quick copy failed'; throw 'Quick copy failed'; }
        window.__preview.copied = text.replaceAll('\n\n', '\n'); return;
      }
      case 'text_extractor_capabilities': return access().advancedAvailable;
      case 'cancel_text_extraction': return;
      case 'text_extractor_show': return;
      case 'record_text_extractor_shortcut': return;
      case 'extract_screen_text': {
        if (args.mode === 'basic') {
          await new Promise(resolve => setTimeout(resolve, scenario === 'basic-pending' ? 10000 : 100));
          if (scenario === 'basic-error') throw 'Offline text recognition is getting ready. Check Settings → Advanced.';
          return scenario === 'basic-empty' ? '' : text.replaceAll('\n\n', '\n');
        }
        if (!access().advancedAvailable) throw 'Choose an available Advanced provider in Settings.';
        await new Promise(resolve => setTimeout(resolve, scenario === 'pending' ? 30000 : scenario === 'demo' ? 6500 : 1600));
        window.__preview.extractionReadyAt = performance.now();
        if (scenario === 'error') throw 'Could not reach OpenAI. Check your connection and try again.';
        return scenario === 'empty' ? '' : text;
      }
      case 'translate_extracted_text': {
        if (!access().advancedAvailable) throw 'Choose an available Advanced provider in Settings.';
        await new Promise(resolve => setTimeout(resolve, scenario === 'translation-pending' ? 30000 : 1600));
        if (scenario === 'translation-error') throw 'Could not reach OpenAI. Your text is unchanged.';
        if (scenario === 'translation-empty') return '';
        const samples = {
          de: 'Ein wenig Raum zum Nachdenken.\n\nGute Ideen beginnen oft mit etwas Kleinem: einer Zeile in einem Buch, einem flüchtigen Gedanken, ein paar Worten, die es wert sind, bewahrt zu werden.\n\nSchaffe Raum für das, was zählt.',
          uk: 'Трохи простору для роздумів.\n\nХороші ідеї часто починаються з чогось маленького: рядка в книжці, випадкової думки, кількох слів, які варто зберегти.\n\nЗвільни місце для того, що має значення.',
        };
        return samples[args.language] || args.text;
      }
      case 'copy_extracted_text': if (scenario === 'clipboard-error') throw 'The clipboard is busy. Try copying again.'; window.__preview.copied = args.text; return;
      case 'close_text_extractor': window.__preview.closed = true; return;
      case 'set_start_at_login': case 'set_tray_icon_mode': case 'set_auto_schedule': return;
      default: throw new Error(`Unexpected fixture command: ${command}`);
    }
  } } };
})();
