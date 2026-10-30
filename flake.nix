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

            nativeBuildInputs = [ pkgs.pkg-config ];

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

            deviceName = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "*Synaptics*";
              description = "Touchpad device name pattern (maps to -n argument). Auto-detects if null.";
            };

            margins = {
              left = lib.mkOption { type = lib.types.nullOr lib.types.int; default = null; };
              right = lib.mkOption { type = lib.types.nullOr lib.types.int; default = null; };
              top = lib.mkOption { type = lib.types.nullOr lib.types.int; default = null; };
              bottom = lib.mkOption { type = lib.types.nullOr lib.types.int; default = null; };
            };

            polygons = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [];
              example = [ "0,0 20,0 10,30" ];
              description = "Custom polygon exclusion zones (percentages 0-100).";
            };

            extraArgs = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [];
              description = "Additional CLI arguments passed directly to unpalm.";
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
              after = [ "systemd-udev-settle.service" ];


              serviceConfig = {
                ExecStart = let
                  args = lib.optionals (cfg.deviceName != null) [ "-n" cfg.deviceName ]
                    ++ lib.optionals (cfg.margins.left != null) [ "--margin-left" (toString cfg.margins.left) ]
                    ++ lib.optionals (cfg.margins.right != null) [ "--margin-right" (toString cfg.margins.right) ]
                    ++ lib.optionals (cfg.margins.top != null) [ "--margin-top" (toString cfg.margins.top) ]
                    ++ lib.optionals (cfg.margins.bottom != null) [ "--margin-bottom" (toString cfg.margins.bottom) ]
                    ++ lib.concatMap (poly: [ "--polygon" poly ]) cfg.polygons
                    ++ cfg.extraArgs;
                in
                  utils.escapeSystemdExecArgs ([ "${cfg.package}/bin/unpalm" ] ++ args);

                User = "unpalm";
                Group = "unpalm";

                Restart = "always";
                RestartSec = "2s";
                
                # Security Hardening
                # Allow systemd to enumerate /dev/input/* nodes freely
                DevicePolicy = "auto";
                DeviceAllow = [
                  "/dev/uinput rwb"
                  "/dev/input/event* rwb"
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