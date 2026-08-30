tmux_bin=$1
socket=$2
buffer=$3
shell=$4
login_name=$5

if "$tmux_bin" -L "$socket" save-buffer -b "$buffer" -; then
    "$tmux_bin" -L "$socket" delete-buffer -b "$buffer" >/dev/null 2>&1 || :
else
    "$tmux_bin" -L "$socket" delete-buffer -b "$buffer" >/dev/null 2>&1 || :
    printf '%s\n' 'deck: could not restore saved shell history' >&2
fi

exec -a "$login_name" "$shell"
