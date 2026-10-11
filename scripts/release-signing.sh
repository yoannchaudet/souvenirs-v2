#!/bin/bash

is_developer_id_application_identity() {
    local identity=$1
    [[ "$identity" == "Developer ID Application: "* ]]
}

codesign_leaf_authority() {
    local details=$1
    sed -n '/^Authority=/{s/^Authority=//;p;q;}' <<<"$details"
}

require_developer_id_application_authority() {
    local component=$1
    local details=$2
    local authority
    authority=$(codesign_leaf_authority "$details")
    [[ "$authority" == "Developer ID Application: "* ]] || {
        echo "$component signature leaf authority is not Developer ID Application: ${authority:-not found}" >&2
        return 1
    }
}
