param(
  [string] $RestartScript = (Join-Path $PSScriptRoot "restart-muninn.ps1")
)

$ErrorActionPreference = "Stop"

function Fail-MuninnMediaProfile {
  param([string] $Message)
  throw "Muninn media profile verification failed: $Message"
}

function Get-AssignmentAst {
  param(
    [System.Management.Automation.Language.Ast] $Ast,
    [string] $VariableName
  )

  $matches = @($Ast.FindAll({
        param($Node)
        if ($Node -isnot [System.Management.Automation.Language.AssignmentStatementAst]) {
          return $false
        }
        $left = $Node.Left
        return $left -is [System.Management.Automation.Language.VariableExpressionAst] -and
          $left.VariablePath.UserPath -eq $VariableName
      }, $true))

  if ($matches.Count -ne 1) {
    Fail-MuninnMediaProfile "expected exactly one assignment to `$$VariableName, found $($matches.Count)"
  }

  return $matches[0]
}

function Get-ConstantAssignmentValue {
  param(
    [System.Management.Automation.Language.Ast] $Ast,
    [string] $VariableName
  )

  $assignment = Get-AssignmentAst -Ast $Ast -VariableName $VariableName
  $right = $assignment.Right
  if ($right -is [System.Management.Automation.Language.CommandExpressionAst]) {
    $right = $right.Expression
  }
  if ($right -isnot [System.Management.Automation.Language.ConstantExpressionAst]) {
    Fail-MuninnMediaProfile "`$$VariableName is not a constant assignment"
  }

  return $right.Value
}

function Assert-TokenPair {
  param(
    [string[]] $Tokens,
    [string] $Name,
    [string] $Value
  )

  for ($index = 0; $index -lt ($Tokens.Count - 1); $index++) {
    if ($Tokens[$index] -eq $Name -and $Tokens[$index + 1] -eq $Value) {
      return
    }
  }

  Fail-MuninnMediaProfile "video proof arguments are missing '$Name $Value'"
}

if (-not (Test-Path -LiteralPath $RestartScript)) {
  Fail-MuninnMediaProfile "restart script not found at $RestartScript"
}

$tokens = $null
$errors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile(
  (Resolve-Path -LiteralPath $RestartScript),
  [ref] $tokens,
  [ref] $errors
)

if ($errors.Count -gt 0) {
  Fail-MuninnMediaProfile (($errors | ForEach-Object { $_.ToString() }) -join "; ")
}

$framerate = Get-ConstantAssignmentValue -Ast $ast -VariableName "videoProofFramerate"
$bitrateKbps = Get-ConstantAssignmentValue -Ast $ast -VariableName "videoProofBitrateKbps"
if ($framerate -ne 30) {
  Fail-MuninnMediaProfile "video proof framerate is $framerate, expected 30"
}
if ($bitrateKbps -ne 12000) {
  Fail-MuninnMediaProfile "video proof bitrate is $bitrateKbps, expected 12000"
}

$videoProofAssignment = Get-AssignmentAst -Ast $ast -VariableName "videoProofArguments"
$stringConstants = @($videoProofAssignment.Right.FindAll({
      param($Node)
      $Node -is [System.Management.Automation.Language.StringConstantExpressionAst]
    }, $true))
$literalTokens = @($stringConstants | ForEach-Object { $_.Value })

Assert-TokenPair -Tokens $literalTokens -Name "-fflags" -Value "nobuffer"
Assert-TokenPair -Tokens $literalTokens -Name "-flags" -Value "low_delay"
Assert-TokenPair -Tokens $literalTokens -Name "-c:v" -Value "h264_nvenc"
Assert-TokenPair -Tokens $literalTokens -Name "-preset" -Value "p1"
Assert-TokenPair -Tokens $literalTokens -Name "-tune" -Value "ull"
Assert-TokenPair -Tokens $literalTokens -Name "-zerolatency" -Value "1"
Assert-TokenPair -Tokens $literalTokens -Name "-bf" -Value "0"
Assert-TokenPair -Tokens $literalTokens -Name "-delay" -Value "0"
Assert-TokenPair -Tokens $literalTokens -Name "-rc" -Value "cbr"
Assert-TokenPair -Tokens $literalTokens -Name "-rc-lookahead" -Value "0"
Assert-TokenPair -Tokens $literalTokens -Name "-forced-idr" -Value "1"

$videoProofText = $videoProofAssignment.Extent.Text
if ($videoProofText -notmatch '\$videoProofVbvKbits') {
  Fail-MuninnMediaProfile "video proof bufsize is not derived from the one-frame VBV budget"
}
if ($videoProofText -match '"p4"|''p4''|24000k') {
  Fail-MuninnMediaProfile "video proof still contains legacy p4 or 24000k latency settings"
}

Write-Host "Muninn media profile verification passed for $RestartScript"
