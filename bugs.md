
- status and stack are basically the same
- submit doesn't set the upstream tracking branch (e.g., subsequent `git push` results in `git push --set-upstream origin toml`)
- add 'auth' command
- remove 'init' command
- better response when submitting with bad token