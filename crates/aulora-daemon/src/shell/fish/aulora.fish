# aulora's shell integration for fish. fish 4 already says when each
# command starts and how it ended (OSC 133, as VS Code and iTerm2 do);
# this adds that for older fish, and says how many jobs run in the
# background or are paused and how each one ended, so aulora can show
# a dev server that exited.

if test (string split . -- $FISH_VERSION)[1] -lt 4
    function __aulora_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end
    function __aulora_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
    end
end

set -g __aulora_watched

function __aulora_jobs
    set -l running 0
    set -l paused 0
    for line in (jobs 2>/dev/null)
        # Job, Group, (CPU,) State, Command
        set -l fields (string split \t -- $line)
        if contains -- running $fields[2..-2]
            set running (math $running + 1)
        else if contains -- stopped $fields[2..-2]
            set paused (math $paused + 1)
        end
    end
    printf '\e]aulora;jobs;%d;%d\a' $running $paused
end

function __aulora_prompt --on-event fish_prompt
    # Watch each job's processes, to say how they end.
    for pid in (jobs -p 2>/dev/null)
        if not contains -- $pid $__aulora_watched
            set -a __aulora_watched $pid
            function __aulora_ended_$pid --on-process-exit $pid
                set -l pid $argv[2]
                set -e __aulora_watched[(contains -i -- $pid $__aulora_watched)]
                functions -e __aulora_ended_$pid
                printf '\e]aulora;job-ended;%d\a' $argv[3]
            end
        end
    end
    __aulora_jobs
    if not test (string split . -- $FISH_VERSION)[1] -lt 4
        return
    end
    printf '\e]133;A\a'
end
