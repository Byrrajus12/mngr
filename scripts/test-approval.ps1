$ErrorActionPreference = "Stop"

$hook = Join-Path $PSScriptRoot "claude-hook.ps1"

function Assert-True {
  param([bool]$Condition, [string]$Message)
  if (-not $Condition) {
    throw $Message
  }
}

function New-TestPayload {
  param(
    [string]$ToolName,
    [string]$HookEventName = "PermissionRequest"
  )

  @{
    session_id = "session-test"
    hook_event_name = $HookEventName
    cwd = (Get-Location).Path
    permission_mode = "default"
    tool_name = $ToolName
    tool_input = @{ command = "echo hi" }
    permission_suggestions = @(
      @{
        type = "addDirectories"
        directories = @((Get-Location).Path)
        destination = "session"
      },
      @{
        type = "setMode"
        mode = "acceptEdits"
        destination = "session"
      }
    )
  } | ConvertTo-Json -Compress -Depth 10
}

function Invoke-TestPayload {
  param(
    [string]$ToolName,
    [string]$HookEventName = "PermissionRequest",
    [string]$Mode = "payload"
  )

  $oldMode = $env:MNGR_HOOK_TEST_MODE
  $env:MNGR_HOOK_TEST_MODE = $Mode
  try {
    $payload = New-TestPayload -ToolName $ToolName -HookEventName $HookEventName
    $output = $payload | powershell.exe -NoProfile -ExecutionPolicy Bypass -File $hook
    Assert-True (-not [string]::IsNullOrWhiteSpace($output)) "expected hook test output"
    return ($output | ConvertFrom-Json)
  } finally {
    $env:MNGR_HOOK_TEST_MODE = $oldMode
  }
}

$permission = Invoke-TestPayload -ToolName "Bash"
$permissionGuid = [guid]::Empty
Assert-True ([guid]::TryParse([string]$permission.request_id, [ref]$permissionGuid)) "PermissionRequest payload should include a GUID request_id"
Assert-True ($permission.hook_event_name -eq "PermissionRequest") "hook_event_name should be preserved"
Assert-True ($permission.permission_mode -eq "default") "permission_mode should be preserved"
Assert-True ($permission.permission_suggestions.Count -eq 2) "permission_suggestions should be preserved"
Assert-True ($permission.permission_suggestions[0].type -eq "addDirectories") "addDirectories suggestion should be preserved"
Assert-True ($permission.permission_suggestions[1].mode -eq "acceptEdits") "setMode suggestion should be preserved"

$preBash = Invoke-TestPayload -ToolName "Bash" -HookEventName "PreToolUse"
Assert-True ([string]::IsNullOrWhiteSpace([string]$preBash.request_id)) "PreToolUse should not get a request_id"
Assert-True ($preBash.hook_event_name -eq "PreToolUse") "PreToolUse should be preserved"

$outputShape = Invoke-TestPayload -ToolName "Bash" -Mode "permission-output"
Assert-True ($outputShape.continue -eq $true) "Permission output should continue"
Assert-True ($outputShape.suppressOutput -eq $true) "Permission output should suppress output"
Assert-True ($outputShape.hookSpecificOutput.hookEventName -eq "PermissionRequest") "Permission output should target PermissionRequest"
Assert-True ($outputShape.hookSpecificOutput.decision.behavior -eq "allow") "Permission output should allow"
Assert-True ($outputShape.hookSpecificOutput.decision.updatedPermissions.Count -eq 2) "Permission output should echo updatedPermissions"
Assert-True ($outputShape.hookSpecificOutput.decision.updatedPermissions[0].type -eq "addDirectories") "updatedPermissions should preserve suggestion objects"

$payload = New-TestPayload -ToolName "Bash" -HookEventName "PostToolUse"
$output = $payload | powershell.exe -NoProfile -ExecutionPolicy Bypass -File $hook
Assert-True ([string]::IsNullOrWhiteSpace($output)) "fire-and-forget hook events should pass through silently"

Write-Host "approval hook tests passed"
