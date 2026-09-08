
filetrail() {
    local _filetrail_command='' _filetrail_skip=0 _filetrail_arg _filetrail_target
    for _filetrail_arg in "$@"; do
        if [ "$_filetrail_skip" = 1 ]; then
            _filetrail_skip=0
            continue
        fi
        case $_filetrail_arg in
            --data-dir) _filetrail_skip=1 ;;
            --data-dir=*|--) ;;
            -h|--help|-V|--version|--print0)
                command @FILETRAIL@ "$@"
                return $?
                ;;
            *)
                if [ -z "$_filetrail_command" ]; then
                    _filetrail_command=$_filetrail_arg
                fi
                ;;
        esac
    done
    if [ "$_filetrail_command" = cd ]; then
        IFS= read -r -d '' _filetrail_target < <(command @FILETRAIL@ "$@" --print0) || return 1
        builtin cd -- "$_filetrail_target"
    else
        command @FILETRAIL@ "$@"
    fi
}
