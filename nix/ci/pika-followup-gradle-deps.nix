{
  pkgs,
  src,
  androidSdk,
  androidJdk,
}:
let
  followupGradle = if androidJdk != null then pkgs.gradle.override { java = androidJdk; } else pkgs.gradle;
in
pkgs.stdenv.mkDerivation (finalAttrs: {
  pname = "pika-followup-gradle-deps";
  version = "0.1.0";
  inherit src;

  nativeBuildInputs = [
    followupGradle
  ];

  mitmCache = followupGradle.fetchDeps {
    pkg = finalAttrs.finalPackage;
    data = ./pika-followup-gradle-deps.json;
  };

  gradleUpdateTask = ":app:compileDebugAndroidTestKotlin";
  dontUseGradleBuild = true;
  dontUseGradleCheck = true;
  __darwinAllowLocalNetworking = true;

  preGradleUpdate = ''
    if [ -z "${androidSdk}/share/android-sdk" ]; then
      echo "missing androidSdk for followup Gradle dependency fetch" >&2
      exit 1
    fi
    if [ -z "${androidJdk}" ]; then
      echo "missing androidJdk for followup Gradle dependency fetch" >&2
      exit 1
    fi

    cd android
    export ANDROID_HOME="${androidSdk}/share/android-sdk"
    export ANDROID_SDK_ROOT="$ANDROID_HOME"
    export HOME="$TMPDIR/pikaci-followup-home"
    export ANDROID_USER_HOME="$HOME/.android"
    export JAVA_HOME="${androidJdk}"
    export PATH="$JAVA_HOME/bin:$PATH"
    export GRADLE_OPTS="''${GRADLE_OPTS:+$GRADLE_OPTS }-Duser.home=$HOME"
    export JAVA_TOOL_OPTIONS="''${JAVA_TOOL_OPTIONS:+$JAVA_TOOL_OPTIONS }-Duser.home=$HOME"
    mkdir -p "$HOME" "$ANDROID_USER_HOME"
    printf 'sdk.dir=%s\n' "$ANDROID_HOME" > local.properties
    unset PIKACI_ANDROID_AAPT2_OVERRIDE
    for candidate in \
      "$ANDROID_HOME/build-tools/35.0.0/aapt2" \
      "$ANDROID_HOME/build-tools/34.0.0/aapt2"
    do
      if [ -x "$candidate" ]; then
        export PIKACI_ANDROID_AAPT2_OVERRIDE="$candidate"
        break
      fi
    done
    if [ -z "''${PIKACI_ANDROID_AAPT2_OVERRIDE:-}" ]; then
      echo "missing SDK aapt2 for followup Gradle dependency fetch" >&2
      exit 1
    fi
    export GRADLE_OPTS="''${GRADLE_OPTS:+$GRADLE_OPTS }-Dorg.gradle.project.android.aapt2FromMavenOverride=$PIKACI_ANDROID_AAPT2_OVERRIDE"
  '';

  installPhase = ''
    mkdir -p "$out"
  '';

  meta.sourceProvenance = with pkgs.lib.sourceTypes; [
    fromSource
    binaryBytecode
  ];
})
