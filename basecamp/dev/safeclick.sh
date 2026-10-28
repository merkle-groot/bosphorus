#!/bin/bash
# safeclick X Y [WAIT]: clicks (points) only if Basecamp is frontmost and the point is inside its
# front window's bounds, then screenshots to /tmp/bc-test/shot.png (1440 px wide).
X=$1; Y=$2
F=$(osascript -e 'tell application "System Events" to get name of first process whose frontmost is true')
[ "$F" = "LogosBasecamp.bin" ] || { echo "ABORT: frontmost is $F"; exit 1; }
read -r WX WY WW WH < <(osascript -e 'tell application "System Events" to tell process "LogosBasecamp.bin" to get {position, size} of front window' | tr -d ',' )
[ -n "$WH" ] || { echo "ABORT: no Basecamp window"; exit 1; }
[ "$X" -ge $((WX + 5)) ] && [ "$X" -le $((WX + WW - 5)) ] && [ "$Y" -ge $((WY + 30)) ] && [ "$Y" -le $((WY + WH - 5)) ] \
  || { echo "ABORT: $X,$Y outside the window ($WX,$WY ${WW}x$WH)"; exit 1; }
/tmp/bc-test/mouse click "$X" "$Y"
sleep "${3:-2}"
screencapture -x /tmp/bc-test/full.png && sips -Z 1440 /tmp/bc-test/full.png --out /tmp/bc-test/shot.png >/dev/null && echo ok
