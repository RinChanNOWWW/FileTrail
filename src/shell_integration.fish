
function filetrail
    set -l filetrail_command ''
    set -l filetrail_skip 0
    for filetrail_arg in $argv
        if test "$filetrail_skip" = 1
            set filetrail_skip 0
            continue
        end
        switch "$filetrail_arg"
            case --data-dir
                set filetrail_skip 1
            case '--data-dir=*' --
            case -h --help -V --version --print0
                command @FILETRAIL@ $argv
                return $status
            case '*'
                if test -z "$filetrail_command"
                    set filetrail_command "$filetrail_arg"
                end
        end
    end
    if test "$filetrail_command" = cd
        set -l filetrail_target (command @FILETRAIL@ $argv --print0 | string split0)
        if test (count $filetrail_target) != 1
            return 1
        end
        builtin cd -- "$filetrail_target"
    else
        command @FILETRAIL@ $argv
    end
end
