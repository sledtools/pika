{ pkgs }:

pkgs.writeScriptBin "pika-cloud-lifecycle" (
  builtins.replaceStrings
    [ "@python3@" ]
    [ "${pkgs.python3}/bin/python3" ]
    (builtins.readFile ./pika-cloud-lifecycle.py.in)
)
