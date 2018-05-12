extern crate cmake;
#[cfg(not(feature = "nolibsnark"))]
extern crate gcc;

fn main() {
    #[cfg(not(feature = "nolibsnark"))]
    {
        let libsnark = cmake::Config::new("depends/libsnark")
            .define("WITH_PROCPS", "OFF")
            .define("CURVE", "ALT_BN128")
            .define("USE_PT_COMPRESSION", "OFF")
            .define("MONTGOMERY_OUTPUT", "ON")
            .define("BINARY_OUTPUT", "ON")
            .build();

        gcc::Build::new()
            .cpp(true)
            .debug(true)
            .flag("-std=c++11")
            .include("./depends/libsnark")
            .include("./depends/libsnark/depends/libff")
            .include("./depends/libsnark/depends/libfqfft")
            .define("CURVE_ALT_BN128", None)
            .file("lib/wraplibsnark.cpp")
            .compile("libwraplibsnark.a");

        gcc::Build::new()
            .cpp(true)
            .debug(true)
            .flag("-std=c++11")
            .include("./depends/libsnark")
            .include("./depends/libsnark/depends/libff")
            .include("./depends/libsnark/depends/libfqfft")
            .define("CURVE_ALT_BN128", None)
            .define("DEBUG", None)
            .file("lib/wraplibsnarkgadgets.cpp")
            .compile("libwraplibsnarkgadgets.a");

        println!("cargo:warning=libsnark installed to {}", libsnark.display());
        println!(
            "cargo:warning=libsnark libs installed to {}",
            libsnark.join("lib").display()
        );
        println!(
            "cargo:rustc-link-search=native={}",
            libsnark.join("lib").display()
        );

        println!("cargo:rustc-link-lib=gmp");
        println!("cargo:rustc-link-lib=gmpxx");
        println!("cargo:rustc-link-lib=static=snark");
        println!("cargo:rustc-link-lib=static=ff");
    }
}
