# PSScriptAnalyzer settings for the packaging scripts (checked in CI: workflows + packaging lint).
@{
    Severity     = @('Error', 'Warning')
    ExcludeRules = @(
        # These are CI/console scripts: Write-Host keeps progress messages out of function return values.
        'PSAvoidUsingWriteHost',
        # Script parameters used only inside nested functions are reported as unused.
        'PSReviewUnusedParameter'
    )
}
