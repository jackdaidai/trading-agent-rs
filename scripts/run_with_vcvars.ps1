param([string]$exe, [string]$args)
$env:PATH = "C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\*\bin\Hostx64\x64;$env:PATH"
$process = Start-Process -FilePath $exe -ArgumentList $args -PassThru -Wait -NoNewWindow
exit $process.ExitCode