#! /bin/bash

# Try and disable job control!
set +m

# Start by including the user's standard configuration
if [ -f "$HOME/.bashrc" ]; then
	source "$HOME/.bashrc"
fi

# Minimalist prompt
PS1="\W $ "

# Aliases that provide nice columnar output
export COLUMNS
export LINES

alias ls="ls -Cw$COLUMNS"