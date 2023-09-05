# Nix builder

Small service that uses [https://nixpacks.com](Nixpacks) to create docker images from git repositories or local paths.

## gettin started 🦀

make sure you have rust and docker installed on ur system

and create a .env file, copy the values from .env.example and fill those in.

now you can build & run the project like this `cargo b` `cargo run`

you should be able to access the server at localhost:8084, and it should show a basic html page.

### trigger an image build
```
{
  "path": "https://github.com/username/repo.git",
  "name": "image-name",
  "envs": ["ENV_VAR1=value", "ENV_VAR2=value"],
  "build_options": {
    "print_dockerfile": false,
    "tags": ["v1.0", "latest"],
    "labels": [],
    "quiet": false,
    "no_cache": false,
    "inline_cache": false,
    "platform": ["linux/amd64"],
    "current_dir": false,
    "no_error_without_start": false,
    "verbose": false
  }
}
```

### Logs Retrieval
To retrieve logs for a specific container, send a GET request to /logs with the following query parameters:

```
container_id: The ID of the container for which you want to retrieve logs.
start_time: The start time of the log collection period in RFC3339 format.
end_time: The end time of the log collection period in RFC3339 format.
```