$ErrorActionPreference = "SilentlyContinue"

$payload = [Console]::In.ReadToEnd()
if ([string]::IsNullOrWhiteSpace($payload)) {
  exit 0
}

$jsonLine = ($payload.Trim() -replace "`r?`n", "")

try {
  $client = [System.IO.Pipes.NamedPipeClientStream]::new(".", "mngr", [System.IO.Pipes.PipeDirection]::Out)
  $client.Connect(50)

  $writer = [System.IO.StreamWriter]::new($client, [System.Text.UTF8Encoding]::new($false))
  $writer.NewLine = "`n"
  $writer.WriteLine($jsonLine)
  $writer.Flush()
  $writer.Dispose()
  $client.Dispose()
} catch {
  exit 0
}

exit 0
