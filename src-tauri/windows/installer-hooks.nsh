!macro NSIS_HOOK_PREINSTALL
  nsExec::ExecToLog '"$INSTDIR\Pulse.exe" --stop-audio-router'
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  nsExec::ExecToLog '"$INSTDIR\Pulse.exe" --stop-audio-router'
  DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Pulse Audio Router"
!macroend
