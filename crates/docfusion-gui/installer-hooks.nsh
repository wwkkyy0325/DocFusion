; DocFusion's installer hooks.
;
; One hook, and only one, because the installer's own work is Tauri's: the
; template removes the program, its shortcuts and its registry keys. What it
; cannot know about is the data this program writes *outside* its install
; directory, and that is what is left over here.
;
; **The template's "delete the application data" checkbox has never deleted
; anything for this program.** It removes `$LOCALAPPDATA\${BUNDLEID}` — the
; bundle identifier, `io.github.wwkkyy0325.docfusion` — while this program keeps
; its files in `$LOCALAPPDATA\DocFusion`; see `docfusion-store/src/dirs.rs`.
; Those were never the same path, so the checkbox was a checkbox that did
; nothing. The hook reads the checkbox rather than inventing a second one, and
; does the part it was always meant to.
;
; The bundler invokes `makensis` with `-INPUTCHARSET UTF8`, so the Chinese below
; is literal text rather than an escape sequence.

!macro NSIS_HOOK_POSTUNINSTALL
  ; Runs after the template has removed files, registry keys and shortcuts.
  ;
  ; `$DeleteAppDataCheckboxState` is the template's own checkbox, and it is 0 in
  ; a silent uninstall because that skips the confirmation page and its
  ; `un.ConfirmLeave`. So `/S` keeps everything — by construction rather than by
  ; accident, which is the only way a rule about deleting somebody's documents
  ; should work.
  ${If} $DeleteAppDataCheckboxState = 1

    ; Models, cache and configuration. None of this is a document: the 144 MB
    ; can be fetched again, and `config.toml` holds an API key, which is a
    ; reason to delete it rather than a reason to hesitate.
    RMDir /r "$LOCALAPPDATA\DocFusion"

    ; The library is a different question and gets asked separately, because it
    ; is a different size of decision. A single checkbox that took a user's
    ; archive along with the cache would put it one misread click away.
    ;
    ; A library kept somewhere else — `--root`, or `DOCFUSION_ROOT` — is not
    ; found and not touched. Guessing where somebody keeps five hundred
    ; documents, and guessing wrong, is a far worse failure than leaving files
    ; on disk.
    IfFileExists "$PROFILE\Documents\DocFusion\*.*" 0 docfusion_library_kept

      StrCpy $R0 "是否一并删除文档库？$\r$\n$\r$\n"
      StrCpy $R0 "$R0$PROFILE\Documents\DocFusion$\r$\n$\r$\n"
      StrCpy $R0 "$R0这里存着 DocFusion 转换出来的 Markdown，以及它读过的源文件副本。$\r$\n"
      StrCpy $R0 "$R0删掉之后无法恢复。$\r$\n"
      StrCpy $R0 "$R0你自己添加的那些源文件不受影响 —— 它们从来没有被复制进这里。$\r$\n$\r$\n"
      StrCpy $R0 "$R0Delete the document library as well? Converted documents in it$\r$\n"
      StrCpy $R0 "$R0cannot be recovered. The files you added yourself are not touched.$\r$\n$\r$\n"
      StrCpy $R0 "$R0默认选择「否」。 The default is No."

      MessageBox MB_YESNO|MB_ICONEXCLAMATION|MB_DEFBUTTON2 "$R0" IDNO docfusion_library_kept

      RMDir /r "$PROFILE\Documents\DocFusion"

    docfusion_library_kept:

  ${EndIf}
!macroend
