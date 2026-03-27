{ nixpkgsLib, serverPkgs }:

{
  system,
  name,
  packageName,
  description,
  variant,
  diskSize,
  modules,
  specialArgs ? { },
  buildInstructions,
  importInstructions,
}:

let
  imageSystem = nixpkgsLib.nixosSystem {
    inherit system modules specialArgs;
  };

  imageDisk = import "${serverPkgs.path}/nixos/lib/make-disk-image.nix" {
    inherit (imageSystem) config pkgs;
    lib = serverPkgs.lib;
    format = "qcow2";
    inherit diskSize name;
    partitionTableType = "efi";
    installBootLoader = true;
    copyChannel = false;
  };
in
serverPkgs.runCommand packageName {
  nativeBuildInputs = [ serverPkgs.findutils serverPkgs.gnutar serverPkgs.xz ];
} ''
  mkdir -p "$out"
  cat >"$TMPDIR/metadata.yaml" <<'EOF'
  architecture: x86_64
  creation_date: 1
  properties:
    description: ${description}
    os: NixOS
    release: unstable
    variant: ${variant}
  EOF
  tar --sort=name --mtime='@1' --owner=0 --group=0 --numeric-owner \
    -C "$TMPDIR" -cJf "$out/metadata.tar.xz" metadata.yaml
  qcow2_path="$(find ${imageDisk} -maxdepth 1 -type f -name '*.qcow2' | head -n 1)"
  if [ -z "$qcow2_path" ]; then
    echo "missing qcow2 image output from ${imageDisk}" >&2
    exit 1
  fi
  ln -s "$qcow2_path" "$out/disk.qcow2"
  cat >"$out/README.txt" <<EOF
  Build:
    ${buildInstructions}

  Import on a remote Incus dev host:
    ${importInstructions}
  EOF
''
