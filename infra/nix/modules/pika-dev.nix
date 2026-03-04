{ config, lib, pkgs, pikaDevPkg, ... }:

let
  servicePort = 8789;
  serviceUser = "pika-dev";
  serviceGroup = "pika-dev";
  serviceStateDir = "/var/lib/pika-dev";
  repoDir = "${serviceStateDir}/repo";
  configFile = pkgs.writeText "pika-dev.toml" ''
    github_repo = "sledtools/pika"
    github_label = "pika-dev"
    github_token_env = "GITHUB_TOKEN"
    poll_interval_secs = 60

    provider = "anthropic"
    model = "claude-sonnet-4-5-20250929"
    thinking_level = "high"
    max_concurrent_sessions = 5
    session_timeout_mins = 30
    workspace_base = "${serviceStateDir}/workspaces"
    session_base = "${serviceStateDir}/sessions"
    agent_backend = "pi"
    base_branch = "origin/master"

    bind_address = "127.0.0.1"
    bind_port = ${toString servicePort}

    allowed_npubs = [
      "npub1dergggklka99wwrs92yz8wdjs952h2ux2ha2ed598ngwu9w7a6fsh9xzpc", # Gigi
      "npub1trr5r2nrpsk6xkjk5a7p6pfcryyt6yzsflwjmz6r7uj7lfkjxxtq78hdpu", # Gladstein
      "npub1qny3tkh0acurzla8x3zy4nhrjz5zd8l9sy9jys09umwng00manysew95gx", # Odell
      "npub1u8lnhlw5usp3t9vmpz60ejpyt649z33hu82wc2hpv6m5xdqmuxhs46turz", # Carman
      "npub15n94rarp3n7dz6edx9cugeshn0kcuxtugwu9nzprkpx7yekw7ygqplqru4", # Clark
      "npub1dlkff8vcdwcty9hs3emc43yks8y7pr0tnn7jewvt63ph077sw48s4cc4qc", # Harrison
      "npub1ye5ptcxfyyxl5vjvdjar2ua3f0hynkjzpx552mu5snj3qmx5pzjscpknpr", # hzrd
      "npub1guh5grefa7vkay4ps6udxg8lrqxg2kgr3qh9n4gduxut64nfxq0q9y6hjy", # Marty
      "npub1l2vyh47mk2p0qlsku7hg0vn29faehy9hy34ygaclpn66ukqp3afqutajft", # pablo
      "npub1p4kg8zxukpym3h20erfa3samj00rm2gt4q5wfuyu3tg0x3jg3gesvncxf8", # Paul
      "npub1330v3vee2mdvn2npdkquka2upmlrlsp0dl2c6kfnzwhpq6dvkjgq0d2k82", # Skyler
      "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y" # justin
    ]
  '';
in
{
  sops.secrets."pika_dev_github_token" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-news.yaml;
    key = "pika_dev_github_token";
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.secrets."pika_dev_anthropic_api_key" = {
    format = "yaml";
    sopsFile = ../../secrets/pika-news.yaml;
    # Reuse existing Anthropic-related secret managed for pika-news.
    key = "pika_news_claude_oauth_token";
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
  };

  sops.templates."pika-dev-env" = {
    owner = serviceUser;
    group = serviceGroup;
    mode = "0400";
    content = ''
      GITHUB_TOKEN=${config.sops.placeholder."pika_dev_github_token"}
      ANTHROPIC_API_KEY=${config.sops.placeholder."pika_dev_anthropic_api_key"}
      RUST_LOG=info
    '';
  };

  users.users."${serviceUser}" = {
    isSystemUser = true;
    group = serviceGroup;
    home = serviceStateDir;
    createHome = true;
  };
  users.groups."${serviceGroup}" = {};

  systemd.services.pika-dev = {
    description = "pika-dev automated issue solver";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" "sops-install-secrets.service" ];
    wants = [ "network-online.target" ];

    path = [ pkgs.git pkgs.gh pkgs.bash ];

    restartTriggers = [
      config.sops.templates."pika-dev-env".path
      pikaDevPkg
      configFile
    ];

    serviceConfig = {
      Type = "simple";
      User = serviceUser;
      Group = serviceGroup;
      WorkingDirectory = repoDir;
      EnvironmentFile = [ config.sops.templates."pika-dev-env".path ];
      ExecStartPre = [
        "${pkgs.bash}/bin/bash -lc 'set -euo pipefail; if [ -n \"${"$"}{GITHUB_TOKEN:-}\" ]; then if ! ${pkgs.gh}/bin/gh auth status -h github.com >/dev/null 2>&1; then printf \"%s\" \"${"$"}GITHUB_TOKEN\" | ${pkgs.gh}/bin/gh auth login --hostname github.com --with-token; fi; ${pkgs.gh}/bin/gh auth setup-git; fi'"
        "${pkgs.bash}/bin/bash -lc 'set -euo pipefail; mkdir -p ${repoDir}; if [ ! -d ${repoDir}/.git ]; then ${pkgs.git}/bin/git clone https://github.com/sledtools/pika.git ${repoDir}; fi'"
        "${pkgs.bash}/bin/bash -lc 'set -euo pipefail; cd ${repoDir}; ${pkgs.git}/bin/git fetch origin; default_branch=$(${pkgs.git}/bin/git remote show origin | ${pkgs.gnused}/bin/sed -n \"s/.*HEAD branch: //p\"); if [ -z \"$default_branch\" ]; then if ${pkgs.git}/bin/git show-ref --verify --quiet refs/remotes/origin/main; then default_branch=main; else default_branch=master; fi; fi; ${pkgs.git}/bin/git checkout \"$default_branch\"; ${pkgs.git}/bin/git pull --ff-only origin \"$default_branch\"'"
      ];
      ExecStart = "${pikaDevPkg}/bin/pika-dev --config ${configFile} --db ${serviceStateDir}/pika-dev.db";
      Restart = "always";
      RestartSec = "5s";

      NoNewPrivileges = true;
      PrivateTmp = true;
      ProtectSystem = "strict";
      ReadWritePaths = [ serviceStateDir ];
    };
  };

  environment.systemPackages = [ pkgs.git pkgs.gh ];

  systemd.tmpfiles.rules = [
    "d ${serviceStateDir} 0750 ${serviceUser} ${serviceGroup} -"
    "d ${repoDir} 0750 ${serviceUser} ${serviceGroup} -"
    "d ${serviceStateDir}/workspaces 0750 ${serviceUser} ${serviceGroup} -"
    "d ${serviceStateDir}/sessions 0750 ${serviceUser} ${serviceGroup} -"
  ];
}
