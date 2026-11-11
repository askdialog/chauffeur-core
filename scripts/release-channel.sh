#!/usr/bin/env bash
# Shared by packaging and smoke tests; runtime cannot override the compiled channel.
if [[ -f "${BASH_SOURCE[0]%/*}/../distribution/build-env.sh" ]]; then
  [[ ${CHAUFFEUR_RELEASE_CHANNEL:-dialog} == dialog ]] || { echo 'Build Core from its public checkout, without distribution/.' >&2; exit 1; }
  CHAUFFEUR_RELEASE_CHANNEL=dialog
else
  [[ ${CHAUFFEUR_RELEASE_CHANNEL:-core} == core ]] || { echo 'Build the Dialog edition from its distribution checkout.' >&2; exit 1; }
  CHAUFFEUR_RELEASE_CHANNEL=core
fi
export CHAUFFEUR_RELEASE_CHANNEL
if [[ $CHAUFFEUR_RELEASE_CHANNEL == core ]]; then
  CHAUFFEUR_DMG_PREFIX=Chauffeur-Core
  CHAUFFEUR_PACKAGE_PREFIX=chauffeur-core
else
  CHAUFFEUR_DMG_PREFIX=Chauffeur
  CHAUFFEUR_PACKAGE_PREFIX=chauffeur
fi
