// LD_PRELOAD shim: intercepts getifaddrs() and returns a fake interface
// with the real IP read from /proc/net/fib_trie or hardcoded fallback.
// Compile: gcc -shared -fPIC -o getifaddrs_shim.so getifaddrs_shim.c

#define _GNU_SOURCE
#include <ifaddrs.h>
#include <net/if.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <dlfcn.h>

// Try to discover real IP from /proc/net/route (reads gateway interface)
// or fall back to connecting a UDP socket to 8.8.8.8 and checking local addr.
static uint32_t get_real_ip(void) {
    // UDP trick: connect to external address, read local side
    int sock = socket(AF_INET, SOCK_DGRAM, 0);
    if (sock < 0) return htonl(0x7f000001); // 127.0.0.1 fallback

    struct sockaddr_in remote = {0};
    remote.sin_family = AF_INET;
    remote.sin_port = htons(53);
    inet_pton(AF_INET, "8.8.8.8", &remote.sin_addr);

    if (connect(sock, (struct sockaddr*)&remote, sizeof(remote)) < 0) {
        close(sock);
        return htonl(0x7f000001);
    }

    struct sockaddr_in local = {0};
    socklen_t len = sizeof(local);
    getsockname(sock, (struct sockaddr*)&local, &len);
    close(sock);

    return local.sin_addr.s_addr;
}

int getifaddrs(struct ifaddrs **ifap) {
    uint32_t ip = get_real_ip();

    struct ifaddrs *ifa = calloc(1, sizeof(struct ifaddrs));
    struct sockaddr_in *addr = calloc(1, sizeof(struct sockaddr_in));
    struct sockaddr_in *mask = calloc(1, sizeof(struct sockaddr_in));

    addr->sin_family = AF_INET;
    addr->sin_addr.s_addr = ip;

    mask->sin_family = AF_INET;
    mask->sin_addr.s_addr = htonl(0xffffff00); // /24

    ifa->ifa_name = "wlan0";
    ifa->ifa_flags = IFF_UP | IFF_RUNNING;
    ifa->ifa_addr = (struct sockaddr*)addr;
    ifa->ifa_netmask = (struct sockaddr*)mask;
    ifa->ifa_next = NULL;

    *ifap = ifa;
    return 0;
}

void freeifaddrs(struct ifaddrs *ifa) {
    if (!ifa) return;
    free(ifa->ifa_addr);
    free(ifa->ifa_netmask);
    free(ifa);
}
