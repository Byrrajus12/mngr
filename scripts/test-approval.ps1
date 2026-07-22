$ErrorActionPreference = "Stop"

$hook = Join-Path $PSScriptRoot "claude-hook.ps1"

function Assert-True {
  param([bool]$Condition, [string]$Message)
  if (-not $Condition) {
    throw $Message
  }
}

function Invoke-TestPayload {
  param(
    [string]$ToolName,
    [string]$HookEventName = "PreToolUse"
  )

  $oldMode = $env:MNGR_HOOK_TEST_MODE
  $env:MNGR_HOOK_TEST_MODE = "payload"
  try {
    $payload = @{
      session_id = "session-test"
      hook_event_name = $HookEventName
      cwd = (Get-Location).Path
      tool_name = $ToolName
      tool_input = @{ command = "echo hi" }
    } | ConvertTo-Json -Compress -Depth 10

    $output = $payload | powershell.exe -NoProfile -ExecutionPolicy Bypass -File $hook
    Assert-True (-not [string]::IsNullOrWhiteSpace($output)) "expected enriched payload output"
    return ($output | ConvertFrom-Json)
  } finally {
    $env:MNGR_HOOK_TEST_MODE = $oldMode
  }
}

$bash = Invoke-TestPayload -ToolName "Bash"
$bashGuid = [guid]::Empty
Assert-True ([guid]::TryParse([string]$bash.request_id, [ref]$bashGuid)) "Bash payload should include a GUID request_id"
Assert-True ($bash.gated -eq $true) "PreToolUse Bash should be gated"
Assert-True ($bash.hook_event_name -eq "PreToolUse") "hook_event_name should be preserved"

$postBash = Invoke-TestPayload -ToolName "Bash" -HookEventName "PostToolUse"
Assert-True ($postBash.gated -eq $false) "PostToolUse Bash should not be gated"

$read = Invoke-TestPayload -ToolName "Read"
$readGuid = [guid]::Empty
Assert-True ([guid]::TryParse([string]$read.request_id, [ref]$readGuid)) "Read payload should include a GUID request_id"
Assert-True ($read.gated -eq $false) "Read should not be gated"

$payload = @{
  session_id = "session-test"
  hook_event_name = "PostToolUse"
  cwd = (Get-Location).Path
  tool_name = "Bash"
  tool_input = @{ command = "echo hi" }
} | ConvertTo-Json -Compress -Depth 10
$output = $payload | powershell.exe -NoProfile -ExecutionPolicy Bypass -File $hook
Assert-True ([string]::IsNullOrWhiteSpace($output)) "non-gated hook events should pass through silently"

Write-Host "approval hook tests passed"
