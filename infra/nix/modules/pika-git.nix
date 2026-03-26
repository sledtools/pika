{ config, lib, pkgs, pikaGitPkg, ... }:

let
  servicePort = 8788;
  serviceUser = "pika-git";
  serviceGroup = "pika-git";
  gitUser = "git";
  serviceStateDir = "/var/lib/pika-git";
  legacyServiceStateDir = "/var/lib/pika-news";
  canonicalGitDir = "${serviceStateDir}/pika.git";
  adminIdentities = import ../lib/admin-identities.nix;
  tomlFormat = pkgs.formats.toml { };
  prepareCanonicalRepo = pkgs.writeShellScript "pika-git-prepare-canonical-repo" ''
    set -euo pipefail
    state_dir=${lib.escapeShellArg serviceStateDir}
    legacy_state_dir=${lib.escapeShellArg legacyServiceStateDir}
    repo=${lib.escapeShellArg canonicalGitDir}

    if [ "$legacy_state_dir" != "$state_dir" ] && [ -d "$legacy_state_dir" ] && [ ! -L "$legacy_state_dir" ]; then
      if [ ! -e "$state_dir" ]; then
        ${pkgs.coreutils}/bin/mv "$legacy_state_dir" "$state_dir"
      elif [ -d "$state_dir" ] && [ -z "$(${pkgs.findutils}/bin/find "$state_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
        ${pkgs.coreutils}/bin/rmdir "$state_dir"
        ${pkgs.coreutils}/bin/mv "$legacy_state_dir" "$state_dir"
      fi
    fi

    ${pkgs.coreutils}/bin/mkdir -p "$state_dir"
    ${pkgs.coreutils}/bin/chown -R ${serviceUser}:${serviceGroup} "$state_dir"

    if [ "$legacy_state_dir" != "$state_dir" ] && [ ! -e "$legacy_state_dir" ]; then
      ${pkgs.coreutils}/bin/ln -s "$state_dir" "$legacy_state_dir"
    fi

    if [ ! -d "$repo" ]; then
      exit 0
    fi

    ${pkgs.coreutils}/bin/chown -R ${gitUser}:${serviceGroup} "$repo"
    ${pkgs.findutils}/bin/find "$repo" -type d -exec ${pkgs.coreutils}/bin/chmod 2775 {} +
    ${pkgs.findutils}/bin/find "$repo" -type f -exec ${pkgs.coreutils}/bin/chmod ug+rw,o-rwx {} +
    if [ -d "$repo/.githooks" ]; then
      ${pkgs.coreutils}/bin/chown -R ${serviceUser}:${serviceGroup} "$repo/.githooks"
      ${pkgs.findutils}/bin/find "$repo/.githooks" -type f -exec ${pkgs.coreutils}/bin/chmod 0775 {} +
    fi
    ${pkgs.coreutils}/bin/chmod 0664 "$repo"/HEAD "$repo"/config "$repo"/description 2>/dev/null || true
    ${pkgs.git}/bin/git --git-dir="$repo" config core.sharedRepository group
  '';
  configFile = tomlFormat.generate "pika-git.toml" {
    repos = [ "sledtools/pika" ];
    forge_repo = {
      repo = "sledtools/pika";
      canonical_git_dir = canonicalGitDir;
      default_branch = "master";
      mirror_remote = "github";
      mirror_poll_interval_secs = 300;
      mirror_timeout_secs = 120;
      hook_url = "http://127.0.0.1:${toString servicePort}/git/webhook";
    };
    poll_interval_secs = 300;
    model = "claude-opus-4-6";
    api_key_env = "ANTHROPIC_API_KEY";
    github_token_env = "GITHUB_TOKEN";
    merged_lookback_hours = 72;
    worker_concurrency = 1;
    bind_address = "127.0.0.1";
    bind_port = servicePort;
    bootstrap_admin_npubs = adminIdentities.prodAdminNpubs;
    allowed_npubs = [ ];
  };
in
{
  sops.secrets."pika_git_github_token" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-git.yaml;
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.secrets."pika_git_claude_oauth_token" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-git.yaml;
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.secrets."pika_git_webhook_secret" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-git.yaml;
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.secrets."pikaci_apple_ssh_key" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-git.yaml;
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.templates."pika-git-env" = {
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
    content = ''
      GITHUB_TOKEN=${config.sops.placeholder."pika_git_github_token"}
      CLAUDE_CODE_OAUTH_TOKEN=${config.sops.placeholder."pika_git_claude_oauth_token"}
      PIKA_GIT_WEBHOOK_SECRET=${config.sops.placeholder."pika_git_webhook_secret"}
      RUST_LOG=info
      PIKACI_APPLE_SSH_KEY_FILE=${config.sops.secrets."pikaci_apple_ssh_key".path}
      PIKACI_APPLE_SSH_HOST=pika-mini.tail029da2.ts.net
      PIKACI_APPLE_SSH_USER=mini
      PIKACI_APPLE_REMOTE_ROOT=/Volumes/pikaci-data/pikaci-apple
      PIKACI_APPLE_KEEP_RUNS=3
      PIKACI_APPLE_LOCK_TIMEOUT_SEC=1800
    '';
  };

  users.users."${serviceUser}" = {
    isSystemUser = true;
    group = serviceGroup;
    home = serviceStateDir;
    createHome = false;
  };

  users.users."${gitUser}" = {
    isSystemUser = true;
    group = serviceGroup;
    home = serviceStateDir;
    createHome = false;
    shell = "${pkgs.git}/bin/git-shell";
  };
  users.groups."${serviceGroup}" = {};

  systemd.services.pika-git = {
    description = "pika git forge service";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" "sops-install-secrets.service" ];
    wants = [ "network-online.target" ];

    path = [
      pkgs.bash
      pkgs.claude-code
      pkgs.curl
      pkgs.git
      pkgs.just
      pkgs.nix
      pkgs.openssh
      pkgs.openssl
      pkgs.python3
      pkgs.rsync
    ];

    restartTriggers = [
      config.sops.templates."pika-git-env".path
      pikaGitPkg
      configFile
    ];

    environment = {
      SSL_CERT_FILE = "/etc/ssl/certs/ca-bundle.crt";
    };

    serviceConfig = {
      Type = "simple";
      User = serviceUser;
      Group = serviceGroup;
      PermissionsStartOnly = true;
      WorkingDirectory = serviceStateDir;
      EnvironmentFile = [ config.sops.templates."pika-git-env".path ];
      ExecStartPre = [ prepareCanonicalRepo ];
      ExecStart = "${pikaGitPkg}/bin/pika-git serve --config ${configFile} --db ${serviceStateDir}/pika-git.db";
      Restart = "always";
      RestartSec = "5s";
      NoNewPrivileges = true;
      PrivateTmp = true;
      ProtectSystem = "strict";
      ReadWritePaths = [ serviceStateDir ];
    };
  };

  nixpkgs.config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) [
    "claude-code"
  ];

  environment.systemPackages = [ pkgs.claude-code ];

  systemd.tmpfiles.rules = [
    "d ${serviceStateDir} 0750 ${serviceUser} ${serviceGroup} -"
    "z ${serviceStateDir} 0750 ${serviceUser} ${serviceGroup} -"
  ];
}
