#define __STDC_WANT_LIB_EXT1__ 1

#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>
#include <Security/cssmapple.h>

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

#define SECRET_INPUT_MAX (128U * 1024U)
#define PKCS12_INPUT_MAX (16U * 1024U * 1024U)

extern OSStatus SecKeychainItemSetAccessWithPassword(
    SecKeychainItemRef item,
    SecAccessRef access,
    UInt32 password_length,
    const void *password
);

static void clear_bytes(void *bytes, size_t length) {
    if (bytes != NULL && length != 0) {
        (void)memset_s(bytes, length, 0, length);
    }
}

static int fail_message(const char *message) {
    fprintf(stderr, "macOS signing keychain helper failed: %s\n", message);
    return 1;
}

static int fail_status(const char *operation, OSStatus status) {
    CFStringRef description = SecCopyErrorMessageString(status, NULL);
    char buffer[512] = {0};
    if (description != NULL) {
        CFStringGetCString(description, buffer, sizeof(buffer), kCFStringEncodingUTF8);
        CFRelease(description);
    }
    fprintf(
        stderr,
        "macOS signing keychain helper failed: %s (%d%s%s)\n",
        operation,
        (int)status,
        buffer[0] == '\0' ? "" : ": ",
        buffer
    );
    return 1;
}

static int read_secret_frame(
    uint8_t *buffer,
    size_t capacity,
    uint8_t **keychain_password,
    size_t *keychain_password_length,
    uint8_t **certificate_password,
    size_t *certificate_password_length,
    size_t *frame_length
) {
    size_t used = 0;
    while (used < capacity) {
        ssize_t count = read(STDIN_FILENO, buffer + used, capacity - used);
        if (count > 0) {
            used += (size_t)count;
            continue;
        }
        if (count == 0) {
            break;
        }
        if (errno == EINTR) {
            continue;
        }
        return fail_message("could not read the secret frame from stdin");
    }
    if (used == capacity) {
        uint8_t extra = 0;
        ssize_t count;
        do {
            count = read(STDIN_FILENO, &extra, 1);
        } while (count < 0 && errno == EINTR);
        if (count != 0) {
            return fail_message("secret frame exceeds the bounded input size");
        }
    }

    uint8_t *first_end = memchr(buffer, '\0', used);
    if (first_end == NULL || first_end == buffer) {
        return fail_message("secret frame has no keychain password");
    }
    size_t first_length = (size_t)(first_end - buffer);
    size_t remaining = used - first_length - 1;
    uint8_t *second = first_end + 1;
    uint8_t *second_end = memchr(second, '\0', remaining);
    if (second_end == NULL || second_end == second) {
        return fail_message("secret frame has no certificate password");
    }
    size_t second_length = (size_t)(second_end - second);
    if ((size_t)(second_end - buffer) + 1 != used) {
        return fail_message("secret frame contains trailing data");
    }

    *keychain_password = buffer;
    *keychain_password_length = first_length;
    *certificate_password = second;
    *certificate_password_length = second_length;
    *frame_length = used;
    return 0;
}

struct pkcs12_buffer {
    uint8_t *bytes;
    size_t length;
    CFDataRef data;
};

static void clear_pkcs12(struct pkcs12_buffer *buffer) {
    if (buffer->data != NULL) {
        CFRelease(buffer->data);
    }
    clear_bytes(buffer->bytes, buffer->length);
    free(buffer->bytes);
    buffer->bytes = NULL;
    buffer->length = 0;
    buffer->data = NULL;
}

static int read_pkcs12(const char *path, struct pkcs12_buffer *buffer) {
    int descriptor = open(path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
    if (descriptor < 0) {
        return fail_message("could not open the PKCS#12 input");
    }

    struct stat metadata;
    if (fstat(descriptor, &metadata) != 0 || !S_ISREG(metadata.st_mode) ||
        metadata.st_size <= 0 || metadata.st_size > PKCS12_INPUT_MAX) {
        close(descriptor);
        return fail_message("PKCS#12 input is not a bounded regular file");
    }

    size_t length = (size_t)metadata.st_size;
    uint8_t *bytes = malloc(length);
    if (bytes == NULL) {
        close(descriptor);
        return fail_message("could not allocate PKCS#12 input storage");
    }

    size_t used = 0;
    while (used < length) {
        ssize_t count = read(descriptor, bytes + used, length - used);
        if (count > 0) {
            used += (size_t)count;
            continue;
        }
        if (count < 0 && errno == EINTR) {
            continue;
        }
        clear_bytes(bytes, length);
        free(bytes);
        close(descriptor);
        return fail_message("could not read the complete PKCS#12 input");
    }
    close(descriptor);

    CFDataRef data = CFDataCreateWithBytesNoCopy(
        kCFAllocatorDefault,
        bytes,
        (CFIndex)length,
        kCFAllocatorNull
    );
    if (data == NULL) {
        clear_bytes(bytes, length);
        free(bytes);
        return fail_message("could not create PKCS#12 input data");
    }
    buffer->bytes = bytes;
    buffer->length = length;
    buffer->data = data;
    return 0;
}

static CFStringRef hex_string(CFDataRef data) {
    static const char digits[] = "0123456789abcdef";
    CFIndex byte_count = CFDataGetLength(data);
    if (byte_count < 0 || byte_count > (CFIndex)(SIZE_MAX / 2U)) {
        return NULL;
    }
    size_t character_count = (size_t)byte_count * 2U;
    char *characters = malloc(character_count + 1U);
    if (characters == NULL) {
        return NULL;
    }
    const UInt8 *bytes = CFDataGetBytePtr(data);
    for (CFIndex index = 0; index < byte_count; ++index) {
        characters[(size_t)index * 2U] = digits[bytes[index] >> 4U];
        characters[(size_t)index * 2U + 1U] = digits[bytes[index] & 0x0fU];
    }
    characters[character_count] = '\0';
    CFStringRef result = CFStringCreateWithBytes(
        kCFAllocatorDefault,
        (const UInt8 *)characters,
        (CFIndex)character_count,
        kCFStringEncodingASCII,
        false
    );
    free(characters);
    return result;
}

static CFStringRef partition_description(void) {
    const void *partition_values[] = {
        CFSTR("apple-tool:"),
        CFSTR("apple:")
    };
    CFArrayRef partitions = CFArrayCreate(
        kCFAllocatorDefault,
        partition_values,
        2,
        &kCFTypeArrayCallBacks
    );
    if (partitions == NULL) {
        return NULL;
    }
    const void *keys[] = {CFSTR("Partitions")};
    const void *values[] = {partitions};
    CFDictionaryRef property = CFDictionaryCreate(
        kCFAllocatorDefault,
        keys,
        values,
        1,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks
    );
    CFRelease(partitions);
    if (property == NULL) {
        return NULL;
    }
    CFErrorRef error = NULL;
    CFDataRef xml = CFPropertyListCreateData(
        kCFAllocatorDefault,
        property,
        kCFPropertyListXMLFormat_v1_0,
        0,
        &error
    );
    CFRelease(property);
    if (error != NULL) {
        CFRelease(error);
    }
    if (xml == NULL) {
        return NULL;
    }
    CFStringRef result = hex_string(xml);
    CFRelease(xml);
    return result;
}

static OSStatus apply_partition_list(
    SecKeychainRef keychain,
    const uint8_t *keychain_password,
    size_t keychain_password_length
) {
    const void *search_values[] = {keychain};
    CFArrayRef search_list = CFArrayCreate(
        kCFAllocatorDefault,
        search_values,
        1,
        &kCFTypeArrayCallBacks
    );
    if (search_list == NULL) {
        return errSecAllocate;
    }

    CFMutableDictionaryRef query = CFDictionaryCreateMutable(
        kCFAllocatorDefault,
        0,
        &kCFTypeDictionaryKeyCallBacks,
        &kCFTypeDictionaryValueCallBacks
    );
    if (query == NULL) {
        CFRelease(search_list);
        return errSecAllocate;
    }
    CFDictionarySetValue(query, kSecClass, kSecClassKey);
    CFDictionarySetValue(query, kSecMatchLimit, kSecMatchLimitAll);
    CFDictionarySetValue(query, kSecReturnRef, kCFBooleanTrue);
    CFDictionarySetValue(query, kSecAttrCanSign, kCFBooleanTrue);
    CFDictionarySetValue(query, kSecMatchSearchList, search_list);

    CFTypeRef matches = NULL;
    OSStatus status = SecItemCopyMatching(query, &matches);
    CFRelease(query);
    CFRelease(search_list);
    if (status != errSecSuccess) {
        return status;
    }
    if (matches == NULL || CFGetTypeID(matches) != CFArrayGetTypeID()) {
        if (matches != NULL) {
            CFRelease(matches);
        }
        return errSecItemNotFound;
    }

    CFStringRef description = partition_description();
    if (description == NULL) {
        CFRelease(matches);
        return errSecAllocate;
    }

    CFIndex key_count = CFArrayGetCount((CFArrayRef)matches);
    for (CFIndex key_index = 0; key_index < key_count; ++key_index) {
        SecKeychainItemRef item = (SecKeychainItemRef)CFArrayGetValueAtIndex(
            (CFArrayRef)matches,
            key_index
        );
        SecAccessRef access = NULL;
        status = SecKeychainItemCopyAccess(item, &access);
        if (status != errSecSuccess) {
            break;
        }
        CFArrayRef acl_list = NULL;
        status = SecAccessCopyACLList(access, &acl_list);
        if (status != errSecSuccess) {
            CFRelease(access);
            break;
        }

        CFIndex acl_count = CFArrayGetCount(acl_list);
        for (CFIndex acl_index = 0; acl_index < acl_count; ++acl_index) {
            SecACLRef acl = (SecACLRef)CFArrayGetValueAtIndex(acl_list, acl_index);
            CSSM_ACL_AUTHORIZATION_TAG tags[64];
            uint32_t tag_count = (uint32_t)(sizeof(tags) / sizeof(tags[0]));
            status = SecACLGetAuthorizations(acl, tags, &tag_count);
            if (status != errSecSuccess) {
                break;
            }
            for (uint32_t tag_index = 0; tag_index < tag_count; ++tag_index) {
                if (tags[tag_index] != CSSM_ACL_AUTHORIZATION_PARTITION_ID) {
                    continue;
                }
                CFArrayRef applications = NULL;
                CFStringRef prompt = NULL;
                CSSM_ACL_KEYCHAIN_PROMPT_SELECTOR selector = {0};
                status = SecACLCopySimpleContents(
                    acl,
                    &applications,
                    &prompt,
                    &selector
                );
                if (status == errSecSuccess) {
                    status = SecACLSetSimpleContents(
                        acl,
                        applications,
                        description,
                        &selector
                    );
                }
                if (applications != NULL) {
                    CFRelease(applications);
                }
                if (prompt != NULL) {
                    CFRelease(prompt);
                }
                if (status != errSecSuccess) {
                    break;
                }
            }
            if (status != errSecSuccess) {
                break;
            }
        }
        CFRelease(acl_list);
        if (status == errSecSuccess) {
            status = SecKeychainItemSetAccessWithPassword(
                item,
                access,
                (UInt32)keychain_password_length,
                keychain_password
            );
        }
        CFRelease(access);
        if (status != errSecSuccess) {
            break;
        }
    }

    CFRelease(description);
    CFRelease(matches);
    return status;
}

static SecAccessRef signing_access(void) {
    SecTrustedApplicationRef codesign = NULL;
    SecTrustedApplicationRef security = NULL;
    OSStatus status = SecTrustedApplicationCreateFromPath("/usr/bin/codesign", &codesign);
    if (status == errSecSuccess) {
        status = SecTrustedApplicationCreateFromPath("/usr/bin/security", &security);
    }
    if (status != errSecSuccess) {
        if (codesign != NULL) {
            CFRelease(codesign);
        }
        if (security != NULL) {
            CFRelease(security);
        }
        fail_status("create trusted signing applications", status);
        return NULL;
    }

    const void *values[] = {codesign, security};
    CFArrayRef applications = CFArrayCreate(
        kCFAllocatorDefault,
        values,
        2,
        &kCFTypeArrayCallBacks
    );
    CFRelease(codesign);
    CFRelease(security);
    if (applications == NULL) {
        fail_message("could not allocate the trusted signing application list");
        return NULL;
    }
    SecAccessRef access = NULL;
    status = SecAccessCreate(CFSTR("Hypercolor signing key"), applications, &access);
    CFRelease(applications);
    if (status != errSecSuccess) {
        fail_status("create signing key access", status);
        return NULL;
    }
    return access;
}

int main(int argc, char **argv) {
    if (argc != 3) {
        return fail_message("usage: macos-signing-keychain <keychain> <certificate.p12>");
    }

    uint8_t secret_frame[SECRET_INPUT_MAX];
    uint8_t *keychain_password = NULL;
    uint8_t *certificate_password = NULL;
    size_t keychain_password_length = 0;
    size_t certificate_password_length = 0;
    size_t frame_length = 0;
    int result = read_secret_frame(
        secret_frame,
        sizeof(secret_frame),
        &keychain_password,
        &keychain_password_length,
        &certificate_password,
        &certificate_password_length,
        &frame_length
    );
    if (result != 0) {
        clear_bytes(secret_frame, sizeof(secret_frame));
        return result;
    }
    if (keychain_password_length > UINT32_MAX) {
        clear_bytes(secret_frame, frame_length);
        return fail_message("keychain password is too large");
    }

    SecKeychainRef keychain = NULL;
    OSStatus status = SecKeychainCreate(
        argv[1],
        (UInt32)keychain_password_length,
        keychain_password,
        false,
        NULL,
        &keychain
    );
    if (status != errSecSuccess) {
        clear_bytes(secret_frame, frame_length);
        return fail_status("create keychain", status);
    }

    SecKeychainSettings settings = {
        .version = SEC_KEYCHAIN_SETTINGS_VERS1,
        .lockOnSleep = true,
        .useLockInterval = true,
        .lockInterval = 21600
    };
    status = SecKeychainSetSettings(keychain, &settings);
    if (status == errSecSuccess) {
        status = SecKeychainUnlock(
            keychain,
            (UInt32)keychain_password_length,
            keychain_password,
            true
        );
    }
    if (status != errSecSuccess) {
        CFRelease(keychain);
        clear_bytes(secret_frame, frame_length);
        return fail_status("configure keychain", status);
    }

    struct pkcs12_buffer pkcs12 = {0};
    if (read_pkcs12(argv[2], &pkcs12) != 0) {
        CFRelease(keychain);
        clear_bytes(secret_frame, frame_length);
        return 1;
    }
    CFStringRef passphrase = CFStringCreateWithBytesNoCopy(
        kCFAllocatorDefault,
        certificate_password,
        (CFIndex)certificate_password_length,
        kCFStringEncodingUTF8,
        false,
        kCFAllocatorNull
    );
    if (passphrase == NULL) {
        clear_pkcs12(&pkcs12);
        CFRelease(keychain);
        clear_bytes(secret_frame, frame_length);
        return fail_message("certificate password is not valid UTF-8");
    }
    SecAccessRef access = signing_access();
    if (access == NULL) {
        CFRelease(passphrase);
        clear_pkcs12(&pkcs12);
        CFRelease(keychain);
        clear_bytes(secret_frame, frame_length);
        return 1;
    }

    SecItemImportExportKeyParameters parameters = {
        .version = SEC_KEY_IMPORT_EXPORT_PARAMS_VERSION,
        .flags = 0,
        .passphrase = passphrase,
        .alertTitle = NULL,
        .alertPrompt = NULL,
        .accessRef = access,
        .keyUsage = NULL,
        .keyAttributes = NULL
    };
    SecExternalFormat format = kSecFormatPKCS12;
    SecExternalItemType item_type = kSecItemTypeAggregate;
    CFArrayRef imported_items = NULL;
    status = SecItemImport(
        pkcs12.data,
        CFSTR("certificate.p12"),
        &format,
        &item_type,
        0,
        &parameters,
        keychain,
        &imported_items
    );
    CFRelease(access);
    CFRelease(passphrase);
    clear_pkcs12(&pkcs12);
    if (imported_items != NULL) {
        CFRelease(imported_items);
    }
    if (status == errSecSuccess) {
        status = apply_partition_list(
            keychain,
            keychain_password,
            keychain_password_length
        );
    }
    CFRelease(keychain);
    clear_bytes(secret_frame, frame_length);
    if (status != errSecSuccess) {
        return fail_status("import and authorize signing identity", status);
    }
    return 0;
}
