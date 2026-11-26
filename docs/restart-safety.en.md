# Shutdown and process cleanup

See [restart-safety.md](restart-safety.md). Confirm exit before canceling tasks. Stop new work, cancel the queue, terminate owned Windows Job Objects, wait up to ten seconds, persist state and restore the terminal. Browser processes are not owned task processes. npm selects versions only at startup; there is no application updater.
