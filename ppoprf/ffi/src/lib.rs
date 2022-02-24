//! Foreign-function interface to the ppoprf randomness implementation
//!
//! This implements a C api so services can easily embed support.
//!

use ppoprf::ppoprf;

/// Opaque struct acts as a handle to the server implementation.
pub struct RandomnessServer {
    inner: ppoprf::Server,
}

/// Construct a new server instance and return an opaque handle to it.
///
/// The handle must be freed by calling randomness_server_release().
// FIXME: Pass a [u8] and length for the md initialization.
#[no_mangle]
pub extern "C" fn randomness_server_create() -> *mut RandomnessServer {
    let test_mds = vec!["t".into()];
    let inner = ppoprf::Server::new(&test_mds);
    let server = Box::new(RandomnessServer { inner });
    Box::into_raw(server)
}

/// Release memory associated with a server instance.
///
/// The handle returned by randomness_server_create() must be passed
/// to this function to release the associated storage.
#[no_mangle]
pub extern "C" fn randomness_server_release(ptr: *mut RandomnessServer) {
    assert!(!ptr.is_null());
    let server = unsafe { Box::from_raw(ptr) };
    drop(server);
}

/// Evaluate the PPOPRF for the given point.
#[no_mangle]
pub extern "C" fn randomness_server_eval(
    ptr: *const RandomnessServer,
    input: *const u8,
    md_index: usize,
    verifiable: bool,
    output: *mut u8,
) {
    // Verify arguments.
    assert!(!ptr.is_null());
    assert!(!input.is_null());
    assert!(!output.is_null());

    // Convert our *const argument to a &ppoprf::Server without taking ownership.
    let server = unsafe { &(*ptr).inner };
    // Wrap the provided compressed Ristretto point in the expected type.
    // Unfortunately from_slice() copies the data here.
    let point = unsafe {
        let bytes = std::slice::from_raw_parts(input, ppoprf::COMPRESSED_POINT_LEN);
        ppoprf::CompressedRistretto::from_slice(bytes)
    };
    // Evaluate the requested point.
    let result = server.eval(&point, md_index, verifiable);
    // Copy the resulting point into the output buffer.
    unsafe {
        std::ptr::copy_nonoverlapping(
            result.output.as_bytes().as_ptr(),
            output,
            ppoprf::COMPRESSED_POINT_LEN,
        );
    }
}

/// Puncture the given md value from the PPOPRF.
#[no_mangle]
pub extern "C" fn randomness_server_puncture(ptr: *mut RandomnessServer, md: u8) {
    // Convert our *const to a &ppoprf::Server without taking ownership.
    assert!(!ptr.is_null());
    let server = unsafe { &mut (*ptr).inner };

    // The ffi signature takes a u8 by value, but the underlying
    // api wants a slice to allow more than 8 bits of metadata tag.
    let md_vec = vec![md];
    // Call correct function.
    server.puncture(&md_vec);
}

#[cfg(test)]
mod tests {
    //! Unit tests for the ppoprf foreign-function interface
    //!
    //! This tests the C-compatible api from Rust for convenience.
    //! Testing it from other langauges is also recommended!

    use crate::*;
    use curve25519_dalek::ristretto::CompressedRistretto;

    #[test]
    /// Verify creation/release of the opaque server handle.
    fn unused_instance() {
        let server = randomness_server_create();
        assert!(!server.is_null());
        randomness_server_release(server);
    }

    #[test]
    /// One evaluation call to the ppoprf.
    fn simple_eval() {
        let server = randomness_server_create();
        assert!(!server.is_null());

        // Evaluate a test point.
        let point = CompressedRistretto::default();
        let mut result = Vec::with_capacity(ppoprf::COMPRESSED_POINT_LEN);
        randomness_server_eval(
            server,
            point.as_bytes().as_ptr(),
            0,
            false,
            result.as_mut_ptr(),
        );
        // FIXME: verify result!
        randomness_server_release(server);
    }

    #[test]
    /// Verify serialization of internal types.
    fn serialization() {
        let point = CompressedRistretto::default();
        println!("{:?}", &point);

        // ppoprf::Evaluation doesn't implement Debug.
    }
}
