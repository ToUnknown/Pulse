(() => {
  const query = new URLSearchParams(location.search);
  const scenario = query.get('scenario') || 'success';
  const text = 'A little space to think.\n\nGood ideas often begin with something small: a line in a book, a passing thought, a few words worth keeping.\n\nMake room for what matters.';
  let settings = { enabled: false, shortcut: 'Control+Shift+KeyE', apiKeyConfigured: query.has('key'), error: null };
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
    ctx.fillStyle = '#e2e8f2'; ctx.font = '14px system-ui'; ctx.fillText('⊞     ⌕     ▣     ◉     ✉', 610, 882); ctx.fillText('21:42', 1360, 876);
    return { width, height, imageUrl: canvas.toDataURL('image/png') };
  };
  window.__TAURI__ = { core: { invoke: async (command, args = {}) => {
    calls.push({ command, args: command === 'save_openai_api_key' ? '[redacted fixture]' : args });
    switch (command) {
      case 'settings_state': return { platform: query.get('platform') || 'windows', startAtLogin: false, trayIcon: 'default', autoSchedule: { lightStart: 7, darkStart: 19 } };
      case 'text_extractor_state': return { ...settings };
      case 'save_openai_api_key': if (scenario === 'key-error') throw 'Could not save the OpenAI key securely.'; settings.apiKeyConfigured = true; return;
      case 'set_text_extractor': if (args.enabled && !settings.apiKeyConfigured) throw 'OpenAI API key required'; if (scenario === 'shortcut-error') throw 'This shortcut is already in use. Choose another combination.'; settings = { ...settings, enabled: args.enabled, shortcut: args.shortcutValue }; return;
      case 'text_extractor_capture': return screenshot();
      case 'text_extractor_show': return;
      case 'record_text_extractor_shortcut': return;
      case 'extract_screen_text': await new Promise(resolve => setTimeout(resolve, scenario === 'pending' ? 30000 : 1600)); if (scenario === 'error') throw 'Could not reach OpenAI. Check your connection and try again.'; return scenario === 'empty' ? '' : text;
      case 'copy_extracted_text': if (scenario === 'clipboard-error') throw 'The clipboard is busy. Try copying again.'; window.__preview.copied = args.text; return;
      case 'close_text_extractor': window.__preview.closed = true; return;
      case 'set_start_at_login': case 'set_tray_icon_mode': case 'set_auto_schedule': return;
      default: throw new Error(`Unexpected fixture command: ${command}`);
    }
  } } };
})();
