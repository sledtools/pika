{ config, lib, pkgs, pikaNewsPkg, ... }:

let
  servicePort = 8788;
  serviceUser = "pika-news";
  serviceGroup = "pika-news";
  gitUser = "git";
  serviceStateDir = "/var/lib/pika-news";
  canonicalGitDir = "${serviceStateDir}/pika.git";
  adminIdentities = import ../lib/admin-identities.nix;
  tomlFormat = pkgs.formats.toml { };
  prepareCanonicalRepo = pkgs.writeShellScript "pika-news-prepare-canonical-repo" ''
    set -euo pipefail
    repo=${lib.escapeShellArg canonicalGitDir}
    if [ ! -d "$repo" ]; then
      exit 0
    fi

    ${pkgs.coreutils}/bin/chown -R ${serviceUser}:${serviceGroup} "$repo"
    ${pkgs.findutils}/bin/find "$repo" -type d -exec ${pkgs.coreutils}/bin/chmod 2775 {} +
    ${pkgs.findutils}/bin/find "$repo" -type f -exec ${pkgs.coreutils}/bin/chmod ug+rw,o-rwx {} +
    if [ -d "$repo/.githooks" ]; then
      ${pkgs.findutils}/bin/find "$repo/.githooks" -type f -exec ${pkgs.coreutils}/bin/chmod 0775 {} +
    fi
    ${pkgs.coreutils}/bin/chmod 0664 "$repo"/HEAD "$repo"/config "$repo"/description 2>/dev/null || true
    ${pkgs.git}/bin/git --git-dir="$repo" config core.sharedRepository group
  '';
  configFile = tomlFormat.generate "pika-news.toml" {
    repos = [ "sledtools/pika" ];
    forge_repo = {
      repo = "sledtools/pika";
      canonical_git_dir = canonicalGitDir;
      default_branch = "master";
      mirror_remote = "github";
      mirror_poll_interval_secs = 300;
      hook_url = "http://127.0.0.1:${toString servicePort}/news/webhook";
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
  sops.secrets."pika_news_github_token" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-news.yaml;
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.secrets."pika_news_claude_oauth_token" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-news.yaml;
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.secrets."pika_news_webhook_secret" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-news.yaml;
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.templates."pika-news-env" = {
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
    content = ''
      GITHUB_TOKEN=${config.sops.placeholder."pika_news_github_token"}
      CLAUDE_CODE_OAUTH_TOKEN=${config.sops.placeholder."pika_news_claude_oauth_token"}
      PIKA_NEWS_WEBHOOK_SECRET=${config.sops.placeholder."pika_news_webhook_secret"}
      RUST_LOG=info
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

  systemd.services.pika-news = {
    description = "pika forge service";
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
      config.sops.templates."pika-news-env".path
      pikaNewsPkg
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
      EnvironmentFile = [ config.sops.templates."pika-news-env".path ];
      ExecStartPre = [ prepareCanonicalRepo ];
      ExecStart = "${pikaNewsPkg}/bin/pika-news serve --config ${configFile} --db ${serviceStateDir}/pika-news.db";
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
