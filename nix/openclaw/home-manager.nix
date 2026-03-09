{ self }:
{ config, lib, pkgs, ... }:

let
  cfg = config.programs.openclaw.pikachatOpenclaw;
in
{
  options.programs.openclaw.pikachatOpenclaw = {
    enable = lib.mkEnableOption "install the pikachat OpenClaw extension and daemon package";

    daemonPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.pikachat;
      description = "Pikachat daemon package placed on PATH for the OpenClaw extension.";
    };

    extensionPackage = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.pikachat-openclaw-extension;
      description = "Packaged OpenClaw extension tree installed under ~/.openclaw/extensions.";
    };

    installDaemonPackage = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Whether to add the pikachat daemon package to home.packages.";
    };
  };

  config = lib.mkIf cfg.enable {
    home.packages = lib.optionals cfg.installDaemonPackage [ cfg.daemonPackage ];

    home.file.".openclaw/extensions/pikachat-openclaw" = {
      source = "${cfg.extensionPackage}/extensions/pikachat-openclaw";
      recursive = true;
    };
  };
}
