# Persistent STA bridge. Only the explicit Windows QA runner starts this process.
# The original clipboard stays in memory and is restored when stdin closes.
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class OcrDesktop {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] struct MOUSEINPUT { public int dx, dy; public uint mouseData, flags, time; public UIntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] struct KEYBDINPUT { public ushort key, scan; public uint flags, time; public UIntPtr extra; }
  [StructLayout(LayoutKind.Explicit)] struct UNION { [FieldOffset(0)] public MOUSEINPUT mouse; [FieldOffset(0)] public KEYBDINPUT keyboard; }
  [StructLayout(LayoutKind.Sequential)] struct INPUT { public uint type; public UNION data; }
  public class Window { public long handle; public uint pid; public string title; public int left, top, width, height; }
  delegate bool EnumProc(IntPtr window, IntPtr param);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback, IntPtr param);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr window, StringBuilder text, int count);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr window, out RECT rect);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint pid);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
  [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr window, int x, int y, int w, int h, bool repaint);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
  [DllImport("user32.dll")] static extern IntPtr WindowFromPoint(POINT point);
  [DllImport("user32.dll")] static extern IntPtr GetAncestor(IntPtr window, uint flags);
  [DllImport("user32.dll")] public static extern short GetAsyncKeyState(int key);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
  [DllImport("user32.dll", SetLastError=true)] static extern uint SendInput(uint count, INPUT[] input, int size);
  public static bool Aborted() { return (GetAsyncKeyState(0x77) & 0x8001) != 0; }
  public static uint ForegroundPid() { uint pid; GetWindowThreadProcessId(GetForegroundWindow(), out pid); return pid; }
  public static long WindowAt(int x, int y) { return GetAncestor(WindowFromPoint(new POINT { X=x, Y=y }), 2).ToInt64(); }
  public static Window[] Windows() {
    var list = new List<Window>();
    EnumWindows((handle, unused) => {
      if (!IsWindowVisible(handle)) return true;
      var title = new StringBuilder(1024); GetWindowText(handle, title, title.Capacity);
      RECT rect; GetWindowRect(handle, out rect); uint pid; GetWindowThreadProcessId(handle, out pid);
      list.Add(new Window { handle=handle.ToInt64(), pid=pid, title=title.ToString(), left=rect.Left, top=rect.Top, width=rect.Right-rect.Left, height=rect.Bottom-rect.Top });
      return true;
    }, IntPtr.Zero); return list.ToArray();
  }
  public static void Key(ushort key, bool up) {
    var input = new INPUT { type=1, data=new UNION { keyboard=new KEYBDINPUT { key=key, flags=up ? 2u : 0u } } };
    if (SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT))) != 1) throw new Exception("Keyboard injection failed");
  }
  public static void Mouse(bool up) {
    var input = new INPUT { type=0, data=new UNION { mouse=new MOUSEINPUT { flags=up ? 4u : 2u } } };
    if (SendInput(1, new[] { input }, Marshal.SizeOf(typeof(INPUT))) != 1) throw new Exception("Mouse injection failed");
  }
}
'@
$null = [OcrDesktop]::SetProcessDpiAwarenessContext([IntPtr](-4))
if ([OcrDesktop]::SetThreadDpiAwarenessContext([IntPtr](-4)) -eq [IntPtr]::Zero) { throw 'Physical desktop coordinate mode is unavailable' }
$savedClipboard = $null
$clipboardSaved = $false
$savedWindow = [OcrDesktop]::GetForegroundWindow()
$savedCursor = New-Object OcrDesktop+POINT
$null = [OcrDesktop]::GetCursorPos([ref]$savedCursor)

function Clipboard-Retry([scriptblock]$action) {
  for ($attempt = 0; $attempt -lt 10; $attempt++) {
    try { return (& $action) } catch {
      if ($attempt -eq 9) { throw }
      Start-Sleep -Milliseconds 50
    }
  }
}
function Check-Abort {
  if ([OcrDesktop]::Aborted()) { throw 'Aborted with F8' }
}
function Restore-Clipboard {
  if ($script:clipboardSaved) {
    Clipboard-Retry {
      if ($null -eq $script:savedClipboard) { [Windows.Forms.Clipboard]::Clear() }
      else { [Windows.Forms.Clipboard]::SetDataObject($script:savedClipboard, $true) }
    }
    $script:clipboardSaved = $false
  }
}
try {
  while ($null -ne ($line = [Console]::ReadLine())) {
    $request = $null
    try {
      $request = $line | ConvertFrom-Json
      if ($request.action -ne 'shutdown') { Check-Abort }
      $result = switch ($request.action) {
        'init' {
          # Materialize formats before replacing the system clipboard. Never log them.
          if ($clipboardSaved) { throw 'Clipboard already saved' }
          $original = Clipboard-Retry { [Windows.Forms.Clipboard]::GetDataObject() }
          if ($null -ne $original) {
            $savedClipboard = New-Object Windows.Forms.DataObject
            foreach ($format in $original.GetFormats($false)) {
              $value = $original.GetData($format, $false)
              if ($null -eq $value) { throw "Cannot preserve clipboard format: $format" }
              if ($value -is [IO.MemoryStream]) { $value = [IO.MemoryStream]::new($value.ToArray()) }
              elseif ($value -is [ICloneable]) { $value = $value.Clone() }
              elseif ($value -isnot [string] -and $value -isnot [ValueType]) {
                throw "Cannot safely preserve clipboard format: $format. Copy plain text first."
              }
              $savedClipboard.SetData($format, $false, $value)
            }
          }
          $clipboardSaved = $true
          @{ clipboardPreserved = $true }
        }
        'state' {
          @{ foregroundPid = [OcrDesktop]::ForegroundPid(); windows = @([OcrDesktop]::Windows());
             monitors = @([Windows.Forms.Screen]::AllScreens | ForEach-Object {
               @{ primary=$_.Primary; name=$_.DeviceName; left=$_.Bounds.Left; top=$_.Bounds.Top; width=$_.Bounds.Width; height=$_.Bounds.Height }
             }); pulsePids = @(Get-Process -Name pulse -ErrorAction SilentlyContinue | ForEach-Object { $_.Id }) }
        }
        'position' {
          if (![OcrDesktop]::MoveWindow([IntPtr]$request.handle, $request.x, $request.y, $request.width, $request.height, $true)) { throw 'Cannot position card window' }
          @{ ok=$true }
        }
        'foreground' {
          $null = [OcrDesktop]::SetForegroundWindow([IntPtr]$request.handle)
          @{ pid=[OcrDesktop]::ForegroundPid() }
        }
        'point' { @{ handle=[OcrDesktop]::WindowAt($request.x, $request.y) } }
        'move' {
          if ([OcrDesktop]::ForegroundPid() -ne $request.pid) { throw 'Foreground changed before moving the cursor' }
          if (![OcrDesktop]::SetCursorPos($request.x, $request.y)) { throw 'Could not move the cursor' }
          @{ ok=$true }
        }
        'hotkey' {
          if ([OcrDesktop]::ForegroundPid() -ne $request.pid) { throw 'Foreground changed before shortcut' }
          $pressed = [System.Collections.Generic.List[System.UInt16]]::new()
          try {
            foreach ($key in $request.keys) { [OcrDesktop]::Key([uint16]$key, $false); $pressed.Add([uint16]$key) }
            Start-Sleep -Milliseconds 35
          } finally { for ($index=$pressed.Count-1; $index -ge 0; $index--) { [OcrDesktop]::Key($pressed[$index], $true) } }
          @{ ok=$true }
        }
        'drag' {
          if ([OcrDesktop]::ForegroundPid() -ne $request.pid) { throw 'Foreground changed before selection' }
          $null = [OcrDesktop]::SetCursorPos($request.x1, $request.y1)
          [OcrDesktop]::Mouse($false)
          try {
            for ($step=1; $step -le 14; $step++) {
              Check-Abort
              if ([OcrDesktop]::ForegroundPid() -ne $request.pid) { throw 'Foreground changed during selection' }
              $null = [OcrDesktop]::SetCursorPos([int]($request.x1 + ($request.x2-$request.x1)*$step/14), [int]($request.y1 + ($request.y2-$request.y1)*$step/14))
              Start-Sleep -Milliseconds 12
            }
          } finally { [OcrDesktop]::Mouse($true) }
          @{ ok=$true }
        }
        'click' {
          if ([OcrDesktop]::ForegroundPid() -ne $request.pid) { throw 'Foreground changed before click' }
          $null = [OcrDesktop]::SetCursorPos($request.x, $request.y)
          [OcrDesktop]::Mouse($false)
          try { Start-Sleep -Milliseconds 30 } finally { [OcrDesktop]::Mouse($true) }
          @{ ok=$true }
        }
        'capture' {
          if ([OcrDesktop]::ForegroundPid() -ne $request.pid) { throw 'Card is not foreground for evidence capture' }
          $bitmap = [Drawing.Bitmap]::new([int]$request.width, [int]$request.height)
          $graphics = [Drawing.Graphics]::FromImage($bitmap)
          try {
            $graphics.CopyFromScreen([int]$request.x, [int]$request.y, 0, 0, $bitmap.Size)
            $bitmap.Save([string]$request.path, [Drawing.Imaging.ImageFormat]::Png)
          } finally { $graphics.Dispose(); $bitmap.Dispose() }
          @{ ok=$true }
        }
        'clipboardSet' {
          if (!$clipboardSaved) { throw 'Clipboard has not been preserved' }
          Clipboard-Retry { [Windows.Forms.Clipboard]::SetText([string]$request.text) }
          @{ ok=$true }
        }
        'clipboardGet' {
          $value = [string](Clipboard-Retry { [Windows.Forms.Clipboard]::GetText() })
          if ($value.Length -gt 10000) { throw 'Unexpected clipboard size; stopped without recording it' }
          @{ text=$value }
        }
        'shutdown' { Restore-Clipboard; @{ clipboardRestored=$true } }
        default { throw 'Unknown desktop action' }
      }
      [Console]::WriteLine((@{ id=$request.id; result=$result } | ConvertTo-Json -Depth 10 -Compress))
      if ($request.action -eq 'shutdown') { break }
    } catch {
      [Console]::WriteLine((@{ id=$request.id; error=$_.Exception.Message } | ConvertTo-Json -Compress))
    }
  }
} finally {
  try { Restore-Clipboard } catch { [Console]::Error.WriteLine('Could not restore the clipboard: ' + $_.Exception.Message) }
  $null = [OcrDesktop]::SetCursorPos($savedCursor.X, $savedCursor.Y)
  $null = [OcrDesktop]::SetForegroundWindow($savedWindow)
}
