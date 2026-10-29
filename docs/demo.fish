#!/usr/bin/env fish
# Regenerates docs/demo.svg; needs asciinema (cargo install --locked asciinema) and npx (svg-term-cli).

set cmd "megafine --jobs 8 --runs 30 --counters 'sleep 0.3' 'sleep 0.4'"
set cols 150
set rows 16

if test "$argv[1]" = play
    echo -n '$ '
    for c in (string split '' -- $cmd)
        echo -n $c
        sleep 0.04
    end
    echo
    eval $cmd
    sleep 4
    exit
end

set self (path resolve (status filename))
cd (status dirname); or exit
set cast (mktemp --suffix=.cast)
asciinema rec \
    --overwrite \
    --output-format asciicast-v2 \
    --window-size {$cols}x{$rows} \
    --command "fish $self play" $cast
and npx --yes svg-term-cli \
    --in $cast \
    --out demo.svg \
    --window \
    --width $cols \
    --height $rows
set ret $status
rm $cast
exit $ret
