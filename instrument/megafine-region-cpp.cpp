// Ancillary binary to exercise `megafine --region` (C++ version).
//
// Takes three sleep durations (seconds) and brackets the middle one with the
// region markers, so `megafine --region` should report ~ the 2nd value:
//
//     sleep(before); megafine_start(); sleep(region); megafine_stop(); sleep(after);
//
// Usage: megafine-region-cpp <before> <region> <after>
//
//     g++ -O2 -Iinstrument instrument/megafine-region-cpp.cpp -o megafine-region-cpp

#include "megafine.h"

#include <chrono>
#include <iostream>
#include <string>
#include <thread>

int main(int argc, char** argv) {
    if (argc != 4) {
        std::cerr << "usage: megafine-region-cpp <before> <region> <after>  (seconds)\n";
        return 2;
    }
    double before = std::stod(argv[1]);
    double region = std::stod(argv[2]);
    double after = std::stod(argv[3]);

    std::this_thread::sleep_for(std::chrono::duration<double>(before));
    megafine_start();
    std::this_thread::sleep_for(std::chrono::duration<double>(region));
    megafine_stop();
    std::this_thread::sleep_for(std::chrono::duration<double>(after));
}
