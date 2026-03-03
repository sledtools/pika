{ config, lib, pkgs, pikaNewsPkg, ... }:

let
  servicePort = 8788;
  serviceUser = "pika-news";
  serviceGroup = "pika-news";
  serviceStateDir = "/var/lib/pika-news";
  configFile = pkgs.writeText "pika-news.toml" ''
    repos = ["sledtools/pika"]
    poll_interval_secs = 300
    model = "claude-sonnet-4-5-20250929"
    api_key_env = "ANTHROPIC_API_KEY"
    github_token_env = "GITHUB_TOKEN"
    merged_lookback_hours = 72
    worker_concurrency = 1
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
      "npub1zxu639qym0esxnn7rzrt48wycmfhdu3e5yvzwx7ja3t84zyc2r8qz8cx2y", # justin
    ]
  '';
in
{
  sops.secrets."pika_news_github_token" = {
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

  systemd.services.pika-news = {
    description = "pika-news PR tutorial feed";
    wantedBy = [ "multi-user.target" ];
    after = [ "network-online.target" "sops-install-secrets.service" ];
    wants = [ "network-online.target" ];

    path = [ pkgs.claude-code ];

    restartTriggers = [
      config.sops.templates."pika-news-env".path
      pikaNewsPkg
      configFile
    ];

    serviceConfig = {
      Type = "simple";
      User = serviceUser;
      Group = serviceGroup;
      WorkingDirectory = serviceStateDir;
      EnvironmentFile = [ config.sops.templates."pika-news-env".path ];
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
  ];
}
