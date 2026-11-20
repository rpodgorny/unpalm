{
  description = "Universal palm rejection for Linux touchpads";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in {
      # 1. The Package Derivation (Rust / Cargo)
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "unpalm";
            version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;

            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            meta = with pkgs.lib; {
              description = "Universal palm rejection for Linux touchpads working at evdev level";
              homepage = "https://github.com/rpodgorny/unpalm";
              license = licenses.gpl3Only;
              platforms = platforms.linux;
              mainProgram = "unpalm";
            };
          };
        });

      # 2. The NixOS System Service Module
      nixosModules.default = { config, lib, pkgs, utils, ... }:
        let
          cfg = config.services.unpalm;
          unpalmPkg = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
        in {

          options.services.unpalm = {
            enable = lib.mkEnableOption "unpalm touchpad filter daemon";

            package = lib.mkOption {
              type = lib.types.package;
              default = unpalmPkg;
              description = "The unpalm package to use.";
            };

            args = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [];
              example = [ "-n" "*Synaptics*" "--margin-left" "12.5" "--block-taps" ];
              description = "CLI arguments passed directly to unpalm (see `unpalm --help`).";
            };
          };

          config = lib.mkIf cfg.enable {
            # Make binary available system-wide
            environment.systemPackages = [ cfg.package ];

            # Static System User & Group (Guarantees NSS lookup success)
            users.users.unpalm = {
              isSystemUser = true;
              group = "unpalm";
              extraGroups = [ "input" "uinput" ];
              description = "Unpalm Touchpad Filter Service User";
            };
            users.groups.unpalm = {};

            # Enable uinput kernel module
            boot.kernelModules = [ "uinput" ];

            # Ensure udev rules allow uinput access
            hardware.uinput.enable = true;

            # Systemd Service Definition
            systemd.services.unpalm = {
              description = "Unpalm Touchpad Filter Service";
              wantedBy = [ "multi-user.target" ];

              serviceConfig = {
                ExecStart = utils.escapeSystemdExecArgs ([ "${cfg.package}/bin/unpalm" ] ++ cfg.args);

                User = "unpalm";
                Group = "unpalm";

                Restart = "always";
                RestartSec = "2s";

                # Security Hardening
                # DeviceAllow is an allowlist: char-input covers /dev/input/*
                # (systemd does not expand globs in DeviceAllow paths).
                DevicePolicy = "auto";
                DeviceAllow = [
                  "/dev/uinput rwb"
                  "char-input rwb"
                ];
                PrivateDevices = false;
                ProtectSystem = "full";
              };
            };
          };
        };
    };
}
