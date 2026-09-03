# The D1 share table, pinned: builds the `share` example in release and runs
# it on one core at High priority, so the in-process best-of-N numbers it
# prints were taken under the discipline in codec-measurement 1 to 3.
#
#   powershell -ExecutionPolicy Bypass -File bench/share.ps1 [-Reps 31] [-Core 4]
#
# -Core is the affinity MASK (4 = the third logical CPU; avoid core 0, it takes
# the interrupts). The example prints its own method line; this wrapper adds
# the pin and priority to it. Nothing here is a speed claim: it is the share
# table the ceiling probe reads, and the null floor that says what a share
# has to clear.
param([int]$Reps = 31, [int]$Core = 4)

$root = Split-Path -Parent $PSScriptRoot
$target = $env:CARGO_TARGET_DIR
if (-not $target) { $target = Join-Path $root 'target' }

Push-Location $root
try {
  cargo build --release --example share -p rusty_esp_dsp
  if ($LASTEXITCODE -ne 0) { Write-Error "build failed"; exit 1 }
} finally { Pop-Location }

$exe = Join-Path $target 'release\examples\share.exe'
if (-not (Test-Path $exe)) { Write-Error "no example binary at $exe"; exit 1 }

$out = Join-Path $env:TEMP ("rusty_esp_dsp-share-{0}.txt" -f [guid]::NewGuid().ToString('N'))
$p = Start-Process -FilePath $exe -ArgumentList @("$Reps") -PassThru -WindowStyle Hidden `
       -RedirectStandardOutput $out
$null = $p.Handle   # cache the handle or TotalProcessorTime reads empty after exit
$p.ProcessorAffinity = [IntPtr]$Core
$p.PriorityClass = 'High'
$p.WaitForExit()
$cpu = $p.TotalProcessorTime.TotalMilliseconds
Get-Content $out
Remove-Item $out -ErrorAction SilentlyContinue
"pin: affinity mask {0}, priority High, process CPU time {1:N0} ms, exit {2}" -f $Core, $cpu, $p.ExitCode
exit $p.ExitCode
