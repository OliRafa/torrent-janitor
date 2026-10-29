# torrent-janitor

Keeps qBittorrent download queues healthy for Sonarr and Radarr. It removes
dead, duplicate, fake and unimportable downloads through the arr, so the arr
blocklists them and searches again within a budget. It also stops downloads
before the disk fills and restarts them once space returns.

Status: planning. See [PLAN.md](PLAN.md).
